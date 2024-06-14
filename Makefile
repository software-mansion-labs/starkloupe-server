deps:
	sh scripts/install-usc.sh \
	&& cd .. \
	&& git clone https://github.com/walnuthq/starknet-foundry.git walnut-starknet-foundry \
	&& cd walnut-starknet-foundry \
	&& git checkout bd6513d56d999a8fddb62b1014578d3dd038768e