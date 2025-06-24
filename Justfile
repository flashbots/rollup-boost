devnet-up: build spawn

devnet-down:
    kurtosis enclave rm -f op-rollup-boost
    kurtosis clean

stress-test:
    chmod +x ./scripts/ci/stress.sh \
        && ./scripts/ci/stress.sh run

build:
    docker buildx build -t flashbots/rollup-boost:develop .

spawn:
    # Pinning to this commit until https://github.com/ethpandaops/optimism-package/issues/349 is fixed
    kurtosis run github.com/ethpandaops/optimism-package@452133367b693e3ba22214a6615c86c60a1efd5e --args-file ./scripts/ci/kurtosis-params.yaml --enclave op-rollup-boost
