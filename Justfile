kurtosis-devnet-up: build kurtosis-spawn

kurtosis-devnet-down:
    kurtosis enclave rm -f op-rollup-boost
    kurtosis clean

kurtosis-stress-test:
    chmod +x ./scripts/ci/kurtosis.sh \
        && ./scripts/ci/kurtosis.sh run

build:
    docker build -t flashbots/rollup-boost:develop .

kurtosis-spawn:
    kurtosis run github.com/ethpandaops/optimism-package --args-file ./scripts/ci/kurtosis-params.yaml --enclave op-rollup-boost