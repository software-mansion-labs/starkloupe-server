use cairo_lang_starknet_classes::contract_class::ContractClass;
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use tracing::{debug, warn};
use uuid::Uuid;
use verification::db::{
    insert_class_hash_profiles, insert_contract_class, insert_verification_request,
};
use verification::gcs::upload_class_to_gcs;
use verification::voyager::CompiledExternalClass;
use walnut_shared::tuple_to_version_string;

/// Persist a Voyager-compiled class to GCS + DB so future requests can skip
/// re-fetching from Voyager and re-compiling. Mirrors what the Walnut
/// verification flow writes for a "real" verification, but tags the synthetic
/// `verification_requests` row with status `"voyager"` so it's distinguishable.
///
/// `source` is a short label used as a log prefix (e.g. `"voyager"` from the
/// inline success path, `"background-retry"` from the retry path).
///
/// Errors are logged, not propagated — partial persistence is acceptable since
/// the next request will re-attempt whatever piece is missing.
pub async fn persist_compiled_voyager_class(
    gcs_client: &google_cloud_storage::client::Storage,
    db_pool: &Pool<Postgres>,
    compiled: &CompiledExternalClass,
    source: &'static str,
) {
    // 1. Inline class (GCS + contract_classes) and, if available, original class
    //    in parallel. Original is the non-inline build matching on-chain CASM.
    let inline_persist = persist_class(
        gcs_client,
        db_pool,
        &compiled.inline_class_hash,
        &compiled.contract_class,
        &compiled.source_code,
        true,
        source,
    );
    match &compiled.original_contract_class {
        Some(original) => {
            let original_persist = persist_class(
                gcs_client,
                db_pool,
                &compiled.original_class_hash,
                original,
                &compiled.source_code,
                false,
                source,
            );
            tokio::join!(inline_persist, original_persist);
        }
        None => inline_persist.await,
    }

    // 2. verification_requests row. The schema requires verification_id on
    //    class_hash_profiles, but Voyager-fetched classes don't go through
    //    the normal verification flow — tag with status="voyager".
    let verification_id = Uuid::new_v4();
    let cairo_version_str = tuple_to_version_string(compiled.cairo_version);
    if let Err(e) = insert_verification_request(
        db_pool,
        verification_id,
        "voyager",
        &cairo_version_str,
        &compiled.package_name,
    )
    .await
    {
        warn!(
            "{}: failed to insert verification_request for {}: {:?}",
            source, compiled.original_class_hash, e
        );
    }

    // 3. class_hash_profiles rows — mirror the Walnut convention (helpers.rs):
    //    Row A: inline class as self-reference (is_inline_strategy_active = true)
    //    Row B: original class with pointer to inline (is_inline_strategy_active = false)
    //    Row B is skipped when no non-inline profile matched on-chain CASM.
    if let Err(e) = insert_class_hash_profiles(
        db_pool,
        &compiled.inline_class_hash,
        &compiled.inline_profile,
        verification_id,
        &true,
        Some(&compiled.inline_class_hash),
    )
    .await
    {
        warn!(
            "{}: failed to insert inline class_hash_profiles for {}: {:?}",
            source, compiled.inline_class_hash, e
        );
    }
    if let (Some(_), Some(orig_profile)) = (
        &compiled.original_contract_class,
        &compiled.original_profile,
    ) {
        if let Err(e) = insert_class_hash_profiles(
            db_pool,
            &compiled.original_class_hash,
            orig_profile,
            verification_id,
            &false,
            Some(&compiled.inline_class_hash),
        )
        .await
        {
            warn!(
                "{}: failed to insert original class_hash_profiles for {}: {:?}",
                source, compiled.original_class_hash, e
            );
        }
    }

    debug!(
        "{}: persisted {} (inline={}) to DB+GCS",
        source, compiled.original_class_hash, compiled.inline_class_hash
    );
}

/// Upload the class to GCS and insert the DB row in parallel. Errors on either
/// side are logged, not propagated — partial persistence is acceptable since
/// the next request will re-attempt the missing piece.
async fn persist_class(
    gcs_client: &google_cloud_storage::client::Storage,
    db_pool: &Pool<Postgres>,
    class_hash: &str,
    contract_class: &ContractClass,
    source_code: &HashMap<String, String>,
    is_cairo_debug_info: bool,
    source: &'static str,
) {
    debug!("{}: uploading class {} to GCS", source, class_hash);
    let gcs_fut = upload_class_to_gcs(gcs_client, class_hash, contract_class, &None, source_code);
    let db_fut = insert_contract_class(db_pool, class_hash, true, is_cairo_debug_info, true, None);
    let (gcs_res, db_res) = tokio::join!(gcs_fut, db_fut);
    if let Err(e) = gcs_res {
        warn!(
            "{}: failed to upload class {} to GCS: {:?}",
            source, class_hash, e
        );
    }
    if let Err(e) = db_res {
        warn!(
            "{}: failed to insert class {} in DB: {:?}",
            source, class_hash, e
        );
    }
}
