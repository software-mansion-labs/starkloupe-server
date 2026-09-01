deps:
	sh scripts/install-usc.sh

postgres:
	cd local && ./start-db-local.sh

run:
	cargo run --bin server

# Postgres + MinIO for the end-to-end simulation test.
e2e-deps:
	docker compose -f local/e2e-docker-compose.yaml up -d --wait postgres minio
	docker compose -f local/e2e-docker-compose.yaml run --rm createbuckets

e2e-deps-down:
	docker compose -f local/e2e-docker-compose.yaml down

# Needs E2E_RPC_URL pointing at a Starknet mainnet RPC serving spec 0.10.
e2e:
	./scripts/e2e-test.sh
