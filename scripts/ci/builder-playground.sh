install_contender() {
    cargo install --git https://github.com/flashbots/contender --bin contender --force
}

# Main execution block
case "$1" in
    "install")
        install
        ;;
    "install-contender")
        install_contender
        ;;
    "run")
        run
        ;;
    "clean")
        clean
        ;;
    *)
        echo "Usage: $0 {install|install-contender|deploy|run|clean}"
        echo "Commands:"
        echo "  install - Install Kurtosis CLI"
        echo "  install-contender - Install Contender"
        echo "  deploy  - Deploy the Optimism package"
        echo "  run     - Run the Optimism package"
        echo "  clean   - Clean up the Kurtosis environment"
        exit 1
        ;;
esac