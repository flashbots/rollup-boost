kurtosis-devnet-up: build kurtosis-spawn

kurtosis-devnet-down:
    kurtosis enclave rm -f op-rollup-boost
    kurtosis clean

kurtosis-stress-test:
    chmod +x ./scripts/ci/kurtosis.sh \
        && ./scripts/ci/kurtosis.sh run

# Build docker image (release)
build:
    docker buildx build --build-arg RELEASE=true -t flashbots/rollup-boost:develop .

# Build docker image (debug)
build-debug:
    docker buildx build --build-arg RELEASE=false -t flashbots/rollup-boost:develop .

kurtosis-spawn:
    kurtosis run github.com/ethpandaops/optimism-package@v1.4.0 --args-file ./scripts/ci/kurtosis-params.yaml --enclave op-rollup-boost
