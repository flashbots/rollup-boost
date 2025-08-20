# Running Rollup Boost Locally

To run a local development network, you can use either Kurtosis or builder-playground to spin up the op-stack with rollup-boost.

## Builder Playground

Builder playground is a tool to deploy an end-to-end block builder environment locally. It can be used to test both L1 and OP Stack block builders.

This will include deploying an OP Stack chain with:

- A complete L1 setup (CL/EL)
- A complete L2 sequencer (op-geth/op-node/op-batcher)
- Op-rbuilder as the external block builder with Flashblocks support

```bash
builder-playground cook opstack --external-builder op-rbuilder
```

Flags:

`--enable-latest-fork` (int): Enables the latest fork (isthmus) at startup (0) or n blocks after genesis.
`--flashblocks`: Enables rollup-boost with Flashblocks enabled for pre-confirmations

In this setup, there is a prefunded test account to send test transactions to:

- address: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
- private key: ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

## Kurtosis

Kurtosis is a tool to manage containerized services. To run rollup-boost in an op-stack devnet, first make sure you have [just](https://github.com/casey/just) and [kurtosis-cli](https://docs.kurtosis.com/install/) installed.

This would include spinning up:

- sequencer `op-node` and `op-geth`
- builder `op-node` and `op-rbuilder`
- rollup boost `rollup-boost`
- ethereum l1 devnet with l2 contracts deployed
- `op-proposer` and `op-batcher`

Then run the following command to start the devnet:

```sh
just devnet-up
```

To stop the devnet run:

```sh
just devnet-down
```

To run a stress test against the devnet with [contender](https://github.com/flashbots/contender), first make sure you have Docker installed.

Then run the following command:

```sh
just stress-test
```

See the [optimism package](https://github.com/ethpandaops/optimism-package/blob/main/README.md#configuration) for additional configuration options.
