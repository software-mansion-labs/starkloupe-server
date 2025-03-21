deps:
	sh scripts/install-usc.sh

postgres:
	cd local && ./start-db-local.sh