deps:
	sh scripts/install-usc.sh \
	&& cd .. \
	&& git clone https://github.com/walnuthq/starknet-foundry.git walnut-starknet-foundry \
	&& cd walnut-starknet-foundry \
	&& git checkout 03410d497dc131923e7e76e8a267a35c42d0a741