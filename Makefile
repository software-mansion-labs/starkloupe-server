deps:
	sh scripts/install-usc.sh \
	&& cd .. \
	&& git clone https://github.com/walnuthq/starknet-foundry.git walnut-starknet-foundry \
	&& cd walnut-starknet-foundry \
	&& git checkout c081e477393ec43ee2912971f39c2fd20762a94a