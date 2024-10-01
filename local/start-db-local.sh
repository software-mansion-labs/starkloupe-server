# Starts the local PostgreSQL database for development using Docker Compose.
docker compose -f db-docker-compose.yaml down
docker compose -f db-docker-compose.yaml up -d