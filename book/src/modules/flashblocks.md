# Flashblocks

Flashblocks is a rollup-boost module that enables fast confirmation times by breaking down block construction into smaller, incremental sections. This feature allows for pre-confirmations and improved user experience through faster block finalization.

## Architecture

The Flashblocks setup consists of three main components:

- **rollup-boost**: The main service with Flashblocks enabled
- **op-rbuilder**: A builder with Flashblocks support
- **op-reth**: A fallback builder (standard EL node)

It utilizes websockets to stream flashblocks from the builder to rollup-boost to minimize the latency between the flashblocks and the sequencer. 

### Flashblocks Workflow

```mermaid
flowchart LR
    subgraph Sequencer
        ON[OP Node]
        RB[Rollup Boost]
        FEL[Fallback EL]
        BB[Block Builder]
    end
    
    subgraph Network
        WSP[WebSocket Proxy]
    end
    
    subgraph Clients
        RPC[RPC Providers]
        Users[End Users]
    end
    
    ON --> RB
    RB --> FEL
    RB <--> BB
    RB --> WSP
    WSP --> RPC
    RPC --> Users
```

1. **Websocket Communication**: Flashblocks utilizes websockets to stream flashblocks from the builder to rollup-boost once its constructed to minimize the latency between the flashblocks and the sequencer. 
2. **Websocket Proxy**: rollup-boost caches these flashblocks and streams them to a websocket proxy. The proxy then fans out the flashblocks to downstream rpc providers.
3. **RPC Overlay**: Once RPC providers receive these flashblocks from the proxy, clients need to run a modified node that supports serving RPC requests with the flashblocks preconfirmation state.
4. **Full Block**: At the end of the slot, the proposer requests a full block from rollup-boost. If rollup-boost does not have the flashblocks cached due to a lost connection, it will fallback to the getPayload call to both the local execution client and the builder.

See the [specs](https://github.com/flashbots/rollup-boost/blob/main/specs/flashblocks.md) for the full design details for flashblocks.