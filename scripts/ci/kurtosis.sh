#!/bin/bash

install() {
    # https://docs.kurtosis.com/install/
    echo "Installing Kurtosis..."
    
    echo "deb [trusted=yes] https://apt.fury.io/kurtosis-tech/ /" | sudo tee /etc/apt/sources.list.d/kurtosis.list
    sudo apt update
    sudo apt install -y kurtosis-cli
    
    # Validate installation
    if which kurtosis > /dev/null; then
        echo "✅ Kurtosis installation completed and verified!"
    else
        echo "❌ Kurtosis installation failed. 'kurtosis' command not found in PATH"
        return 1
    fi
}

run() {
    # Run the kurtosis optimism package
    echo "Running Kurtosis..."
    kurtosis analytics disable
    kurtosis run github.com/ethpandaops/optimism-package --args-file ./scripts/ci/kurtosis-params.yaml
}

clean() {
    # Clean up the kurtosis environment
    echo "Cleaning up..."
    kurtosis clean -a
}

# Main execution block
case "$1" in
    "install")
        install
        ;;
    "run")
        run
        ;;
    "clean")
        clean
        ;;
    *)
        echo "Usage: $0 {install|run|clean}"
        echo "Commands:"
        echo "  install - Install Kurtosis CLI"
        echo "  run     - Run the Optimism package"
        echo "  clean   - Clean up the Kurtosis environment"
        exit 1
        ;;
esac
