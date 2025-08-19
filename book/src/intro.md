# Rollup Boost

Rollup Boost is a lightweight sidecar for rollups that enables rollup extensions. These extensions allow for an efficient, decentralized, and verifiable block building platform for rollups.

## What is Rollup Boost?

[Rollup Boost](https://github.com/flashbots/rollup-boost/) uses the [Engine API](https://specs.optimism.io/protocol/exec-engine.html#engine-api) in the Optimism stack to enable its rollup extensions. This requires no modification to the OP stack software and enables rollup operators to connect to an external builder.

It is designed and developed by [Flashbots](https://flashbots.net/) under a MIT license. We invite developers, rollup operators, and researchers to join in developing this open-source software to achieve efficiency and decentralization in the rollup ecosystem.

## What are the design goals of Rollup Boost?

**Simplicity**: Rollup Boost was designed with minimal complexity, and maintaining a lightweight setup avoids additional latency. By focusing on simplicity, Rollup Boost integrates smoothly into the OP stack without disrupting existing op-node and execution engine communication.

**Modularity**: Recognizing that rollups have varying needs, Rollup Boost was built with extensibility in mind. It is designed to support custom block-building features, allowing operators to implement modifications for any variety of block selection or customization rules.

**Liveness Protection**: To safeguard against liveness risks, Rollup Boost offers a local block production fallback if there is a failure in the external builder.

**Compatibility**: Rollup Boost allows operators to continue using standard op-node or op-geth software without any custom forks.

See this [doc](https://github.com/flashbots/rollup-boost/blob/main/docs/design-philosophy-testing-strategy.md) for the design philosophy and testing strategy for Rollup Boost.

## Is it secure?

Rollup Boost underwent a security audit on May 11, 2025, with [Nethermind](https://www.nethermind.io). See the full report [here](https://github.com/flashbots/rollup-boost/blob/main/docs/NM_0411_0491_Security_Review_World_Rollup_Boost.pdf).

In addition to the audit, we have an extensive suite of integration and kurtosis tests in our CI to ensure the whole flow works end-to-end with the OP stack as well as with external builders.

## Who is this for?

Rollup Boost is designed for:

- **Rollup Operators**: Teams running OP Stack or compatible rollups who want to improve efficiency, reduce confirmation times, and have more control over block construction.
- **Rollup Developers**: Engineers building on rollups to unlock new use cases with rollup extensions.
- **Block Builders**: Teams focused on MEV who want to offer specialized block building services for rollups.
- **Researchers**: Those exploring new approaches to MEV and block production strategies.

## Getting Help

- GitHub Issues: Open an [issue](https://github.com/flashbots/rollup-boost/issues/new) for bugs or feature requests
- Forum: Join the [forum](https://collective.flashbots.net/c/rollup-boost) for development updates and research topics