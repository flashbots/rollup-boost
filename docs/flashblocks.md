# Flashblocks

Flashblocks is a feature in rollup-boost that enables pre-confirmations by proposing incremental sections of blocks. This guide walks you through setting up a complete Flashblocks environment with rollup-boost, op-rbuilder, and a fallback builder.

## Overview

The setup consists of three main components:

- **rollup-boost**: The main service with Flashblocks enabled
- **op-rbuilder**: A builder with Flashblocks support
- **op-reth**: A fallback builder (standard EL node)

## Prerequisites

- Rust toolchain installed
- Access to the rollup-boost and op-rbuilder repositories

## Setup Instructions

### 1. Start rollup-boost with Flashblocks

Launch rollup-boost with Flashblocks enabled:

```bash
cargo run --bin rollup-boost -- \
  --l2-url http://localhost:5555 \
  --builder-url http://localhost:4445 \
  --l2-jwt-token 688f5d737bad920bdfb2fc2f488d6b6209eebda1dae949a8de91398d932c517a \
  --builder-jwt-token 688f5d737bad920bdfb2fc2f488d6b6209eebda1dae949a8de91398d932c517a \
  --rpc-port 4444 \
  --flashblocks \
  --log-level info
```

### 2. Generate Genesis Configuration

Navigate to the op-rbuilder directory and create a genesis file:

```bash
cd op-rbuilder
cargo run -p op-rbuilder --bin tester --features testing -- genesis > genesis.json
```

### 3. Start the op-rbuilder

Launch op-rbuilder with Flashblocks enabled using the generated genesis file:

```bash
cargo run --bin op-rbuilder -- node \
  --chain genesis.json \
  --datadir data-builder \
  --port 3030 \
  --flashblocks.enabled \
  --disable-discovery \
  --authrpc.port 4445 \
  --authrpc.jwtsecret c \
  --http
```

**Note**: The JWT token is located at `./crates/op-rbuilder/src/tests/framework/artifacts/test-jwt-secret.txt` and matches the configuration used in rollup-boost.

### 4. Start the Fallback Builder

Launch op-reth as a fallback builder:

```bash
op-reth node \
  --chain genesis.json \
  --datadir one \
  --port 3131 \
  --authrpc.port 5555 \
  --disable-discovery \
  --authrpc.jwtsecret ./crates/op-rbuilder/src/tests/framework/artifacts/test-jwt-secret.txt
```

This runs a standard op-reth execution layer node that serves as the fallback builder for rollup-boost.

### 5. Simulate the Consensus Layer

Use the built-in tester utility to simulate a consensus layer node:

```bash
cargo run -p op-rbuilder --bin tester --features testing -- run
```
