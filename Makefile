deps:
	curl -L https://raw.githubusercontent.com/software-mansion/universal-sierra-compiler/master/scripts/install.sh | sh \
	&& cd .. \
	&& git clone https://github.com/walnuthq/starknet-foundry.git walnut-starknet-foundry \
	&& cd walnut-starknet-foundry \
	&& git checkout 0c83a684b3033f4c8efb288812715ce7dfa59fc4