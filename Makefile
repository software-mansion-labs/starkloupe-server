deps:
	sh scripts/install-usc.sh \
	&& cd .. \
	&& git clone https://github.com/walnuthq/starknet-foundry.git walnut-starknet-foundry \
	&& cd walnut-starknet-foundry \
	&& git checkout 6c21a7f6042475182e6ac1b9fd40590cd3a39d74