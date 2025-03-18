# Rollup Boost - Load Testing Acceptance Criteria

## Overview
This document serves as a generic acceptance criteria checklist for load testing a **block builder** on a Layer 2 network. It's designed to be used with [Contender](https://github.com/flashbots/contender), a high-performance Ethereum network testing tool for benchmarking and stress-testing clients and networks. Copy this checklist and fill it out with the appropriate data for your specific chain before proceeding to mainnet deployment.

## Environment
- **Network:** [Specify the blockchain network]
- **RPC:** [Specify the blockchain network]
- **Contender Scenarios:** [Specify scenarios used for load testing]
- **Block Builder Version:** [Github link to release]
- **OP Stack Version:** [Github link to release]
- **Test Period:** [YYYY-MM-DD]

## Acceptance Criteria Checklist

### **1. Transaction Throughput & Latency**
✅ **Peak Transactions per Second (TPS)**
   - Expected: ≥ X,XXX TPS
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Average Transaction Latency**
   - Expected: ≤ XXX ms
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Max Transaction Latency (P99)**
   - Expected: ≤ XXX ms
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **RPC Response Latency**
   - Expected: ≤ XXX ms
   - Measured: `____`
   - Supporting Data: [Link to results]

### **2. Block Production Performance**
✅ **Average Block Time**
   - Expected: ≤ XX seconds
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Missed Blocks Rate**
   - Expected: ≤ X% of total blocks
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Internal Block Builder Latency**
   - Expected: ≤ XXX ms
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Latency from Chain RPC to Block Builder**
   - Expected: ≤ XXX ms
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **100% Transaction Forwarding Acceptance**
   - Expected: All transactions sent should be logged in both the chain operator's environment and the block builder's environment
   - Measured: `____`
   - Supporting Data: [Link to results]

### **3. Instance Health & Deployment Readiness**
✅ **Chain Operator Health Checks** *(CPU, Memory, Disk, Networking)*
   - Expected: No resource exhaustion or anomalies
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Block Builder Health Checks** *(CPU, Memory, Disk, Networking)*
   - Expected: No resource exhaustion or anomalies
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **RPC Endpoint Stability**
   - Expected: No downtime or crashes
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Able to Trace Transaction Hash End-to-End**
   - Expected: Transactions can be fully traced from submission to inclusion in a block
   - Measured: `____`
   - Supporting Data: [Link to results]

✅ **Verification of Non-Tx Related RPC Methods**
   - Expected: No failures in non-transaction-related RPC calls
   - Measured: `____`
   - Supporting Data: [Link to results]

---

## **Final Decision**
✅ **All criteria met (Yes/No)?**  
   - Decision: `____`
   - Date: `____`

## **Notes & Recommendations**
- `[Add additional observations, issues, or follow-ups]`