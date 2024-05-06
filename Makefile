deps:
	sh scripts/install-usc.sh \
	&& cd .. \
	&& git clone https://github.com/walnuthq/starknet-foundry.git walnut-starknet-foundry \
	&& cd walnut-starknet-foundry \
	&& git checkout a7faee3307c45141e39105127aa0fd5941d0b1fe