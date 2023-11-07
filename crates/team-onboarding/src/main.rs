use prefixed_api_key::PrefixedApiKeyController;
use sha2::Sha256;

fn main() {
    let builder_result = PrefixedApiKeyController::<_, Sha256>::configure()
        .prefix("walnut".to_owned())
        .rng_osrng()
        .short_token_length(8)
        .long_token_length(16)
        .finalize();

    assert!(builder_result.is_ok());

    let mut controller = builder_result.unwrap();

    // Generate a new PrefixedApiKey
    let (pak, hash) = controller.generate_key_and_hash();

    // Assert that the returned key matches the hash
    assert!(controller.check_hash(&pak, &hash));

    // Stringify the key to be sent to the user. This creates a string from the
    // PrefixedApiKey which follows the `<prefix>_<short token>_<long token>` convention
    let pak_string = pak.to_string();

    println!("Api key: {}", pak_string)
}
