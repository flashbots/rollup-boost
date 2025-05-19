 Flashtestations: Transparent Onchain TEE Verification and Curated Allowlist Protocol

## Table of Contents
- [Introduction](#introduction)
- [Design Goals](#design-goals)
- [System Architecture](#system-architecture)
- [Terminology](#terminology)
  - [Intel TDX Primitives](#intel-tdx-primitives)
  - [Flashtestations Protocol Components](#flashtestations-protocol-components)
  - [Operational Terms](#operational-terms)
- [Data Structures](#data-structures)
  - [TDXQuote](#tdxquote)
  - [TDReport](#tdreport)
  - [DCAPEndorsements](#dcapendorsements)
  - [TDXMeasurements](#tdxmeasurements)
- [TEE Attestation Mechanism](#tee-attestation-mechanism)
  - [Intel TDX DCAP Attestation](#intel-tdx-dcap-attestation)
  - [Onchain DCAP Attestation](#onchain-dcap-attestation)
  - [Workload Identity Derivation](#workload-identity-derivation)
- [Allowlist Registry](#allowlist-registry)
  - [Core Concepts](#core-concepts)
  - [Key Relationship Model](#key-relationship-model)
  - [Fundamental Operations](#fundamental-operations)
  - [Key Requirements](#key-requirements)
- [Policy Layer: Flexible Authorization](#policy-layer-flexible-authorization)
  - [Policy Abstraction](#policy-abstraction)
  - [Policy Operations](#policy-operations)
- [End-to-End Flow](#end-to-end-flow)
  - [Attestation and Registration](#attestation-and-registration)
  - [Runtime Authorization](#runtime-authorization)
  - [Maintenance: Handling Changing Endorsements](#maintenance-handling-changing-endorsements)
- [Offchain TEE Address Verification](#offchain-tee-address-verification)
- [Transparency Log](#transparency-log)
  - [Purpose and Benefits](#purpose-and-benefits)
  - [Logged Information](#logged-information)
  - [Implementation Approach](#implementation-approach)
  - [Relationship with Allowlist](#relationship-with-allowlist)
- [Design Considerations](#design-considerations)
  - [Replacement Model](#replacement-model)
  - [Gas Optimization](#gas-optimization)
  - [Separation of Concerns](#separation-of-concerns)
- [Reproducible Builds](#reproducible-builds)

## Introduction

Trusted Execution Environments (TEEs) offer a promising approach for running confidential workloads with hardware-enforced security guarantees. However, integrating TEEs with blockchain applications presents significant challenges: How can smart contracts verify that they're interacting with authentic TEE services running expected code? How can this verification scale efficiently onchain? How can we maintain an up-to-date registry of validated services as hardware security requirements evolve?

Flashtestations addresses these challenges by providing a comprehensive onchain protocol for TEE verification, address registration, and transparent record-keeping. The protocol enables:

1. Onchain verification of Intel TDX attestations against current Intel endorsements
2. Maintenance of a curated allowlist of validated Ethereum addresses associated with specific TEE workloads
3. Policy-based authorization for smart contracts to securely interact with TEE services
4. Transparent logging of all attestation events and endorsement changes

## Prerequisites

This document assumes familiarity with the following background material, specifications, and tooling. Items are arranged in the rough order they become relevant while reading this spec:

1. **Intel TDX Architecture & Security Model** — core concepts, measurement registers, Trust Domain isolation, and attestation flows.
   • *Key reference:* [Intel TDX Specifications and Developer Guides](https://www.intel.com/content/www/us/en/developer/tools/trust-domain-extensions/documentation.html)
2. **Intel DCAP Attestation Stack** — Quote generation, signature scheme and collateral (QE Identity & TCB Info) retrieval.
   – [Intel TDX DCAP Quoting Library API](https://download.01.org/intel-sgx/latest/dcap-latest/linux/docs/Intel_TDX_DCAP_Quoting_Library_API.pdf)
3. **On‑Chain DCAP Quote Verification** — Solidity contracts that decode DCAP quotes and perform cryptographic validation using PCCS‑sourced endorsements.
   • [Automata DCAP Attestation Contract](https://github.com/automata-network/automata-dcap-attestation)
4. **On‑Chain Endorsement Storage (PCCS)** — Automata’s Solidity implementation that mirrors Intel collateral on Ethereum, enabling fully reproducible verification.
   • [Automata On‑chain PCCS](https://github.com/automata-network/automata-on-chain-pccs)

## Design Goals

Flashtestations is designed to achieve the following objectives:

1. **Security**: Provide cryptographic proof that a service is running inside a genuine TEE with expected code, with verification resistant to spoofing or replay attacks.

2. **Efficiency**: Ensure key operations (especially allowlist lookups) are gas-efficient enough for regular use in smart contracts, with O(1) complexity regardless of allowlist size.

3. **Maintainability**: Support efficient updates as Intel endorsements evolve, without requiring re-verification of all attestations.

4. **Flexibility**: Enable policy-based access control that can adapt to different trust requirements without modifying consumer contracts.

5. **Transparency**: Maintain auditable records of all attestations and endorsement changes to support accountability and security analysis.

6. **Separation of Concerns**: Clearly separate allowlist mechanics from policy governance, enabling independent evolution of these components.

## System Architecture

The Flashtestations protocol consists of four key components that work together to provide secure onchain TEE verification:

```
┌─────────────────────┐                  ┌─────────────────────┐
│ TDX VM              │                  │ Onchain Verifier    │
│                     │  Attestation     │                     │
│ ┌─────────────────┐ │  Quote           │ ┌─────────────────┐ │
│ │ TEE Workload    │ │ ───────────────► │ │ DCAP Attestation│ │
│ │                 │ │                  │ │ Verifier        │ │
│ │ (workloadId)    │ │                  │ │                 │ │
│ └─────────────────┘ │                  │ └────────┬────────┘ │
│                     │                  │          │          │
└─────────────────────┘                  │          ▼          │
                                         │ ┌─────────────────┐ │
┌─────────────────────┐                  │ │ Intel           │ │
│ Consumer Contract   │                  │ │ Endorsements    │ │
│                     │                  │ │ (tcbHash)       │ │
│ ┌─────────────────┐ │                  │ └────────┬────────┘ │
│ │ Operation       │ │                  │          │          │
│ │ Authorization   │ │                  │          ▼          │
│ └─────────────────┘ │                  │ ┌─────────────────┐ │
│         │           │                  │ │ Registration    │ │
└─────────┼───────────┘                  │ │ Logic           │ │
          │                              │ └────────┬────────┘ │
          │                              └──────────┼──────────┘
          │                                         │
┌─────────▼───────────┐                             ▼
│ Policy Registry     │                  ┌───────────────────────┐
│                     │  isAllowed       │ Allowlist Registry    │
│ ┌─────────────────┐ │  Query           │                       │
│ │ workloadId[]    │ │ ◄───────────────►│ (workloadId,address)  │
│ │ per policyId    │ │                  │ mappings              │
│ └─────────────────┘ │                  │                       │
│                     │                  └───────────────────────┘
└─────────────────────┘
```

1. **Onchain Verifier**: Validates TDX attestation quotes against current Intel endorsements
2. **Allowlist Registry**: Tracks which addresses have valid attestations for specific workloads
3. **Policy Registry**: Defines which workloads are acceptable for specific onchain interactions
4. **Transparency Log**: Records all attestations and endorsement changes (implemented via events)

## Terminology

The terms in this section are used consistently throughout the specification documents. When a term is first mentioned elsewhere, it links back here.

### Intel TDX Primitives

**Trusted Execution Environment (TEE)**: Hardware-based isolated execution environment that protects code and data from the host operating system and other applications. In Intel TDX, the isolation boundary is the "Trust Domain" (TD) rather than the bare CPU.

**Intel TDX ([Trust Domain Extensions](https://www.intel.com/content/www/us/en/developer/tools/trust-domain-extensions/documentation.html))**: Intel's TEE technology for virtual machines that provides hardware-enforced isolation, integrity verification, and attestation capabilities. TDX creates isolated Trust Domains (TDs) inside virtual machines.

**Attestation**: The cryptographic process by which a TEE proves its identity and integrity to a verifier. Produces a signed structure (Quote) containing measurements and claims about the TEE's state.

**DCAP ([Data Center Attestation Primitives](https://download.01.org/intel-sgx/latest/dcap-latest/linux/docs/Intel_TDX_DCAP_Quoting_Library_API.pdf))**: Intel's attestation system designed for data centers that enables verification without requiring direct communication with Intel for each attestation check.

**Quote**: The cryptographically signed data structure produced during attestation, containing measurement registers and report data fields that uniquely identify the TEE and its contents. Flashtestations supports the DCAP v5 Quote format.

**Intel DCAP endorsements**: Data provided by Intel that serves as the trust anchor for attestation verification. This includes QE Identity information, TCB Info, certificates, and related data. Also referred to as "Endorsements" in some contexts.

**Collateral**: See Intel DCAP endorsements. It carries the same meaning. This is not monetary collateral as in crypto-economic systems. Some sources, such as Automatas [onchain PCCS](https://github.com/automata-network/automata-on-chain-pccs) uses collateral as the go to term.

**TCB (Trusted Computing Base)**: The set of hardware, firmware, and software components critical to a system's security. In TDX, the TCB includes Intel CPU hardware, microcode, and firmware components that must be at approved security levels.

**Measurement Registers**: Hardware-enforced registers within the TEE (MRTD, RTMRs, MROWNER, etc.) that capture cryptographic hashes of code, data, and configuration loaded into the environment. These registers form the basis for workload identity.

**REPORTDATA**: A 64-byte field in the attestation quote containing user-defined data. In Flashtestations, this contains a the public part of an Ethereum address key pair that the TEE workload controls.

**Quote Enclave (QE)**: Intel-provided enclave responsible for signing attestation quotes using Intel-provisioned keys. The QE creates the cryptographic binding between measurement registers and the attestation signature.

**Provisioning Certification Service (PCS)**: Intel's service that provides the certificates and related data needed to verify attestation quotes. In Flashtestations, we use Automata's onchain PCCS, which stores this data on the blockchain.

**Attestation Key (AK)**: The cryptographic key used by the Quote Enclave to sign attestation quotes. The validity of this key is established through a certificate chain back to Intel.

### Flashtestations Protocol Components

**`workloadId`**: A 32-byte hash uniquely identifying a specific TEE workload based on its measurement registers. Derived as keccak256(MRTD || RTMR[0..3] || MROWNER || MROWNERCONFIG || MRCONFIGID).

**`tcbHash`**: A 32-byte hash representing a specific Intel DCAP endorsements bundle at a point in time. This is a Flashtestations-calculated keccak256 hash of the endorsements components obtained from Automata's onchain PCCS, not a value provided directly by Intel.

**Allowlist Registry**: The onchain data structure that tracks which Ethereum addresses have been validated for specific workloads based on successful attestation. Implemented as the TdxAllowlist contract.

**Policy Registry**: A mapping system that groups related workload identities under a single policy identifier, enabling flexible authorization rules without modifying consumer contracts.

**Transparency Log**: The onchain event-based system that records all attestation verifications, allowlist changes, and endorsement updates for auditability. Implemented through emitted blockchain events rather than as a separate logging service.

**Onchain Verifier**: The smart contract component (using Automata's DCAP attestation system) that validates TDX attestation quotes against current Intel DCAP endorsements and interacts with the Allowlist Registry to register validated addresses.

**Workload**: The specific software running inside a TEE. Its identity is derived from measurement registers that contain cryptographic hashes of loaded code and configuration.

**`policyId`**: An identifier that maps to a list of approved `workloadId`s, enabling contracts to reference policies rather than specific workloads.

**PCCS (Provisioning Certificate Caching Service)**: Automata's onchain implementation of Intel's PCCS that stores Intel DCAP endorsements on the blockchain, making it available for attestation verification. This ensures all verification is reproducible on L2.

### Operational Terms

**Registration**: The process of adding an Ethereum address to the allowlist after successful attestation verification.

**Endorsement Revocation**: The process of marking a specific `tcbHash` as insecure and removing corresponding addresses from the allowlist when Intel updates its security requirements.

**Housekeeping**: The maintenance process of removing addresses from the allowlist when their validating endorsements become stale or insecure. Activated when governance calls `removeEndorsement(workloadId, tcbHash)`.

**TCB Recovery**: The process that occurs when Intel releases updates to address security vulnerabilities in the TCB components. This typically requires updating the list of secure endorsements.

**Reproducible Build**: A deterministic build process that ensures anyone building the same source code will produce identical binary outputs, enabling verification of expected TEE measurements.

## Data Structures

The protocol defines several key data structures:

### `TDXQuote`

The output of the Intel TDX attestation process.

```python
class TDXQuote():
    Header: QuoteHeader
    TDReport: TDReport
    TEEExtendedProductID: uint16
    TEESecurityVersion: uint16
    QESecurityVersion: uint16
    QEVendorID: Bytes16
    UserData: Bytes64
    Signature: Bytes
```

**Field descriptions:**

- `Header`: Version and attestation key type information.
- `TDReport`: TD measurement registers.
- `TEEExtendedProductID`: TEE product identifier.
- `TEESecurityVersion`: Security patch level of the TEE.
- `QESecurityVersion`: Security version of the Quoting Enclave.
- `QEVendorID`: Vendor ID of the Quoting Enclave (Intel).
- `UserData`: User-defined data included in the quote (e.g., public key).
- `Signature`: ECDSA signature over the Quote.

### `TDReport`

Contains the measurement registers and report data from the TEE.

```python
class TDReport():
    MRTD: Bytes48
    RTMR: List[Bytes48]  // Size 4
    MROWNER: Bytes48
    MRCONFIGID: Bytes48
    MROWNER_CONFIG: Bytes48
    ReportData: Bytes64
```

**Field descriptions:**

- `MRTD`: Measurement register for the TD (initial code/data).
- `RTMR`: Runtime measurement registers.
- `MROWNER`: Measurement register for the owner (policy).
- `MRCONFIGID`: Configuration ID.
- `MROWNER_CONFIG`: Owner-defined configuration.
- `ReportData`: User-defined data included in the report (e.g., public key hash).

### `DCAPEndorsements`

Data provided by Intel to verify the authenticity of a TDX Quote.

```python
class DCAPEndorsements():
    QEIdentity: Bytes
    TCBInfo: Bytes
    QECertificationData: Bytes
```

**Field descriptions:**

- `QEIdentity`: Quoting Enclave Identity.
- `TCBInfo`: Trusted Computing Base information.
- `QECertificationData`: Certification data for the attestation key.

### `TDXMeasurements`

A structured representation of the TDX measurement registers.

```python
class TDXMeasurements():
    MRTD: Bytes
    RTMR: List[Bytes]  // Size 4
    MROWNER: Bytes
    MRCONFIGID: Bytes
    MROWNERCONFIG: Bytes
```

**Field descriptions:**

- `MRTD`: Initial TD measurement (boot loader, initial data).
- `RTMR`: Runtime measurements (extended at runtime).
- `MROWNER`: Contains the operator's public key (Ethereum address or other identifier).
- `MRCONFIGID`: Hash of service configuration stored onchain and fetched on boot.
- `MROWNERCONFIG`: Contains unique instance ID chosen by the operator.

## TEE Attestation Mechanism

Attestation is the process by which a TEE proves its identity and integrity. The protocol uses Intel TDX with DCAP (Data Center Attestation Primitives) attestation.

### Intel TDX DCAP Attestation

TDX attestation produces a Quote structure as defined in the [TDXQuote](#tdxquote) and [TDReport](#tdreport) sections.

The attestation process follows these steps:

1. The TEE generates a TD Report containing its measurement registers and report data
2. The Quote Enclave (QE) creates a Quote by signing the TD Report with an Attestation Key
3. The Quote can be verified against Intel's Provisioning Certification Service (PCS)

### Onchain DCAP Attestation

The following code sample illustrates how DCAP attestation verification is performed onchain, and how the key components (workloadId, tcbHash, and Ethereum address) are extracted and registered in the Flashtestations allowlist:

```solidity
// Sample interaction with Automata DCAP Attestation
function registerTEEService(bytes calldata rawQuote) {
    // Verify the DCAP quote onchain using Automata's verifier
    // Note: The verifier internally checks the quote against current endorsements
    bool isValid = IDCAPAttestation(DCAP_ATTESTATION_CONTRACT).verifyAndAttestOnChain(rawQuote);
    require(isValid, "Invalid DCAP quote");
    
    // Extract and convert address from quote's report data
    address ethAddress = extractAddressFromQuote(rawQuote);
    
    // Extract workload identity from quote measurements
    bytes32 workloadId = extractWorkloadIdFromQuote(rawQuote);

    // Get the current endorsement hash (tcbHash) that was used for verification
    // This represents the specific Intel endorsements bundle that validated this quote
    bytes32 tcbHash = IDCAPAttestation(DCAP_ATTESTATION_CONTRACT).getCurrentEndorsementHash();
    
    // Register the address in the allowlist
    IAllowlist(ALLOWLIST_CONTRACT).addAddress(workloadId, tcbHash, ethAddress);
    
    emit TEEServiceRegistered(workloadId, tcbHash, ethAddress);
}
```

This implementation highlights several key aspects:
1. The DCAP attestation is verified using Automata's onchain verifier
2. The workloadId is derived from the quote's measurement registers
3. The Ethereum address is extracted from the quote's report data
4. The tcbHash representing the current Intel endorsements is obtained
5. The extracted information is registered in the allowlist

### Workload Identity Derivation

A TEE's workload identity is derived from a combination of its measurement registers. The TDX platform provides several registers that capture different aspects of the workload through the [TDXMeasurements](#tdxmeasurements) structure.

The workload identity computation takes these registers into account:

```
keccak256(abi.encodePacked(MRTD, RTMR0, RTMR1, RTMR2, RTMR3, MROWNER, MROWNERCONFIG, MRCONFIGID)))
```

These measurement registers serve specific purposes in the permissioned attestation model:

- **MROWNER**: Contains the operator's public key (Ethereum address or other identifier), establishing who is authorized to run this instance
- **MROWNERCONFIG**: Contains a unique instance ID chosen by the operator, which the operator must sign to authenticate itself
- **MRCONFIGID**: Contains a hash of the actual service configuration that is stored onchain and fetched during boot

All of these values are captured in the workload identity hash, ensuring that any change to the code, configuration, or operator results in a different identity that must be explicitly authorized through governance.

## Allowlist Registry

The Allowlist Registry is a core component of Flashtestations that acts as a bookkeeper for tracking which Ethereum addresses have successfully passed attestation within a Trusted Execution Environment (TEE).

### Core Concepts

At its most abstract level, the Allowlist Registry is responsible for:

1. **Storing addresses** that have been validated through attestation
2. **Associating addresses** with their specific workload identity
3. **Tracking which endorsement bundle** validated each address
4. **Providing efficient lookup** capabilities to verify if an address is authorized

The registry operates on these key abstractions:

1. **Workload Identity (`workloadId`)**: A 32-byte hash derived from TDX measurement registers (as defined in [Workload Identity Derivation](#workload-identity-derivation)) that uniquely identifies a specific piece of code running in a TDX environment. This serves as the primary namespace under which addresses are stored.

2. **Intel Endorsements (`tcbHash`)**: A unique identifier representing Intel's opinion at a specific point in time about which hardware and firmware configurations are secure. Conceptually, the `tcbHash` is a keccak256 hash derived from the endorsements:

   ```
   tcbHash = keccak256(DCAPEndorsements)
   ```

   These endorsements (described in [DCAP Attestation Endorsements](#dcap-attestation-endorsements)) change periodically as Intel releases updates or discovers vulnerabilities. 
   
   **Note:** This is an abstract representation - the actual implementation will need to adhere to the way Automata's onchain PCCS system describes and updates endorsements/endorsements, which involves more complex data structures and lifecycle management.
   
   **Note 2:** Another thing which isn't addressed yet is how we can remove tcbhash entries from the allow list. They will need to be removed if the actual endorsements gets stale. The remove method needs to check this, so we need a tcbHash -> endorsements mapping. This will likely be to expensive to maintain onchain, but keeping the offchain mapping and passing it to the remove method should be possible.

3. **Ethereum Address**: The public key extracted from the attestation's report data field ([TDReport.ReportData](#tdreport)), which will be used to interact with onchain contracts.

### Key Relationship Model

The Allowlist Registry maintains a strict relationship between these entities:

1. For each `(address, workloadId)` pair, there is exactly **one** associated `tcbHash`
2. Each address can be registered for multiple different workloads
3. Many addresses can share the same workloadId
4. Many addresses can be validated against the same tcbHash

This means each entry in the allowlist is a unique combination of `(address, workloadId, tcbHash)`. If an address re-registers for the same workloadId with a newer endorsement, the old entry is replaced rather than maintaining multiple entries.

### Fundamental Operations

The Allowlist Registry provides these core operations:

#### 1. Lookup

The most frequent operation is checking if an address is valid for a specific workload:

```
function isAllowedWorkload(workloadId, address) → boolean
```

This simply checks if the address is currently associated with the specified workload. This operation must be highly gas-efficient as it may run on every block.

#### 2. Registration

When an attestation successfully passes verification:

```
function addAddress(workloadId, tcbHash, address, quote)
```

This operation:
1. Records that this address has been validated for this workload
2. Associates it with the specific endorsement bundle (`tcbHash`) that validated it
3. Stores the raw attestation quote for future reference and verification
3. If the address was previously registered for this workloadId with a different tcbHash, the old entry is replaced

#### 3. Endorsement Management

When Intel endorsements become stale or insecure:

```
function removeEndorsement(tcbHash, endorsement)
```

This operation handles the case where a specific endorsement bundle is no longer considered secure:
1. All addresses registered under this specific tcbHash combination are removed completely
2. This ensures only addresses with current, valid endorsements remain in the allowlist
3. We're passing in the endorsement for verifcation purposes, i.e. map the endorsement to the tcbHash, check if the endorsement has been marked insecure, and only then proceed.

#### 4. Quote Retrieval

To retrieve the stored attestation quote for a specific address:

```
function getQuoteForAddress(address) → bytes
```

This operation returns the raw attestation quote that was used to register the address, enabling offline verification or additional analysis of the attestation data.

### Key Requirements

1. **Single Current Endorsement**: An address has exactly one current endorsement bundle for each workloadId it's registered under.
2. **Complete Revocation**: When an endorsement bundle becomes invalid, all addresses registered with that specific (workloadId, tcbHash) combination are removed completely - there is no partial revocation.
3. **Quote Storage**: The system maintains a copy of the most recent attestation quote used to register each address, supporting external verification and auditability.

4. **Gas Efficiency**: The lookup operation must be extremely efficient (O(1)) regardless of the number of addresses stored.

## Policy Layer: Flexible Authorization

The Policy layer sits above the Allowlist Registry and provides a more flexible authorization mechanism.

### Policy Abstraction

A Policy is simply a named group of workload identities:

```
PolicyId → [WorkloadId₁, WorkloadId₂, ..., WorkloadIdₙ]
```

This abstraction allows contracts to reference a policy (e.g., "L2-BlockBuilding-Production") rather than specific workloads, enabling governance to update which workloads are acceptable without modifying contract code.

### Policy Operations

The Policy layer provides these operations:

```
// Check if an address is allowed under any workload in a policy
function isAllowedPolicy(policyId, address) → boolean

// Governance operations
function addWorkloadToPolicy(policyId, workloadId)
function removeWorkloadFromPolicy(policyId, workloadId)
```

The key function `isAllowedPolicy` checks if an address is valid for ANY of the workloads in the policy group. Conceptually:

```
function isAllowedPolicy(policyId, address) {
  workloadIds = getWorkloadsForPolicy(policyId);
  
  for each workloadId in workloadIds {
    if (isAllowedWorkload(workloadId, address)) {
      return true;
    }
  }
  
  return false;
}
```

## End-to-End Flow

The complete verification flow connects attestation, the allowlist, and the policy layer:

### Attestation and Registration

1. **TEE Environment**: A workload runs in a TDX environment and generates an attestation quote
   - The attestation contains measurement registers (determining the `workloadId` as described in [Workload Identity Derivation](#workload-identity-derivation))
   - The report data field contains an Ethereum public key

2. **Verification Service**: An onchain verification service validates this attestation
   - Checks cryptographic signatures
   - Validates against current Intel endorsements ([DCAPEndorsements](#dcapendorsements))
   - Extracts the Ethereum address and workload measurements

3. **Allowlist Registration**: Upon successful verification, the address is registered
   ```
   allowlist.addAddress(derivedWorkloadId, currentTcbHash, extractedAddress)
   ```
   - If the address was previously registered for this workloadId, the old entry is replaced

### Runtime Authorization

When a contract needs to verify if an operation is authorized:

1. The contract checks if the sender is allowed under a specific policy:
   ```
   if (allowlist.isAllowedPolicy(POLICY_ID, msg.sender)) {
     // Permit the operation
   }
   ```

2. This policy check determines if the address is allowed for any workload in the policy.

### Maintenance: Handling Changing Endorsements

Intel endorsements change over time, requiring a maintenance process:

1. **New Endorsements**: When Intel publishes new endorsements:
   - These are represented by a new `tcbHash`
   - Future attestations are verified against these endorsements
   - Addresses that re-attest will have their allowlist entries updated with the new tcbHash

2. **Stale Endorsements**: When Intel marks certain configurations as insecure:
   - The system must remove all addresses registered under the stale (workloadId, tcbHash) combinations
   - This maintenance operation keeps the allowlist in sync with Intel's current security opinions

3. **Housekeeping Challenge**: 
   - The system must efficiently track which (workloadId, tcbHash) combinations are no longer valid
   - When endorsements change, it must remove all addresses associated with those specific combinations
   - This avoids the expensive approach of re-verifying all attestations

This maintenance process ensures that at any point in time, the allowlist only contains addresses that would pass attestation against currently valid Intel endorsements.

## Offchain TEE Address Verification

The Flashtestations protocol enables comprehensive offchain verification of TEE service addresses through its quote storage mechanism. Applications can retrieve the original attestation quote for any registered address via the getQuoteForAddress(address) function, allowing for complete independent verification without incurring gas costs. This approach permits offchain entities to perform the same cryptographic validation as the original onchain verifier, including measurement verification and endorsement checks against the Intel PCS.

### Example Verification Flow

```javascript
// JavaScript example for offchain quote verification
async function verifyTEEAddressOffchain(serviceAddress) {
  const allowlist = new ethers.Contract(ALLOWLIST_ADDRESS, ALLOWLIST_ABI, provider);
  // Retrieve the stored attestation quote
  const quote = await allowlist.getQuoteForAddress(serviceAddress);
  // Verify the quote against Intel endorsements using local DCAP verification
  return verifyDCAPQuoteLocally(quote, serviceAddress);
}
```

## Transparency Log

The L2 blockchain functions as a transparency log within Flashtestations, maintaining a permanent record of attestation events and their verification. This log provides auditability, verifiability, and transparency for the entire TEE attestation ecosystem.

### Purpose and Benefits

The transparency log serves several critical functions:
1. **Public Verifiability**: Anyone can independently verify that attestations were properly validated
2. **Historical Tracking**: Provides a complete history of which TEEs were registered when, and under which endorsements
3. **Audit Trail**: Creates an immutable record that can be used for forensic analysis or compliance
4. **Endorsement Evolution**: Tracks how Intel's hardware/firmware security evaluations change over time

### Logged Information

The transparency log captures raw attestation data along with verification results:

1. **Raw Attestation Quotes**: The complete DCAP quotes submitted for verification
2. **Intel Endorsements**: The actual endorsement data (endorsements) used to validate attestations
3. **Verification Events**: Records of successful and failed attestation attempts
4. **Endorsement Updates**: Records of when new Intel endorsements are published or old ones revoked

### Implementation Approach

The transparency log is implemented through a combination of blockchain events and calldata storage:

```solidity
// Event definitions for the transparency log
event AttestationSubmitted(
    bytes indexed rawQuote,
    bytes32 indexed workloadId,
    bytes32 indexed tcbHash,
    address ethAddress,
    bool success
);

event EndorsementUpdated(
    bytes32 indexed tcbHash,
    bytes rawEndorsementData,
    bool isValid
);

event AllowlistUpdated(
    bytes32 indexed workloadId,
    bytes32 indexed tcbHash,
    address indexed ethAddress,
    bool isAdded
);

event QuoteStored(
    address indexed ethAddress,
    bytes quote
);
```

When an attestation is verified, the raw quote data is included in the transaction calldata, making it permanently available onchain. The verification results and extracted data are then emitted as events for efficient indexing and querying.

### Relationship with Allowlist

While the allowlist registry maintains the current authorized state (which addresses are currently valid for which workloads), the transparency log maintains the complete history of how that state evolved:

1. **Allowlist**: Optimized for efficient runtime checks and state updates
2. **Transparency Log**: Optimized for auditability and historical verification

This dual approach enables efficient onchain operations while maintaining complete transparency and auditability.

## Design Considerations

### Replacement Model

The conceptual model uses a direct replacement approach:

- When an address re-attests for a workloadId, its old entry is replaced with the new one
- This keeps the model simpler by ensuring each address has exactly one current endorsement per workloadId
- When endorsements become invalid, all addresses using that specific endorsement are removed completely

### Gas Optimization

The system must optimize for gas efficiency, particularly for the lookup operations:

- Lookups should be O(1) regardless of the number of addresses
- Storage slots should be fully cleared when no longer needed (to receive gas refunds)
- Batch operations should be supported for removing addresses when endorsements become invalid

### Separation of Concerns

The design maintains clear separation between:

1. **Allowlist Registry**: Tracks which addresses have attestations validated by current endorsements
2. **Policy Registry**: Defines which workloads are acceptable for specific onchain operations
3. **Verification Service**: Validates attestations and updates the allowlist
4. **Consumer Contracts**: Use policy checks to authorize operations

This separation enables each component to evolve independently, with governance focused on the appropriate level of abstraction.

The Allowlist Registry also provides direct access to stored attestation quotes, allowing external systems to perform their own verification or analysis without requiring additional onchain transactions.

## Reproducible Builds

To establish trust in expected measurements, the TEE block builder must be built using a reproducible build process:

1. **Source Code Publication**: The full source code is published with a specific commit hash
2. **Build Environment**: A deterministic build environment is defined (specific compiler versions, dependencies, etc.)
3. **Build Instructions**: Step-by-step instructions to reproduce the build are published
4. **Verification**: Independent parties can follow the build process and verify that it produces the same measurements

This allows anyone to verify that the expected measurements correspond to the published source code.

## Block Builder TEE Proofs

The Flashtestations protocol can be extended to provide cryptographic guarantees that blocks were constructed by an authorized TEE-based block builder. This section describes how block builders running in a TEE can prove block authenticity through an onchain verification mechanism.

### Core Mechanism

The block builder TEE proof system works through a final transaction appended to each block. This transaction:

1. Calls a designated smart contract method that accepts a block content hash
2. Verifies the caller's authorization using `isAllowedPolicy(block_builder_policy_id, msg.sender)`
3. Provides cryptographic evidence that the block was constructed by a valid TEE-based block builder

The key insight is that the required private key to sign this transaction is protected within the TEE environment. Thus, only a genuine TEE-based block builder with the proper attestation can successfully execute this transaction.

### Block Building Process

When building a block, the TEE block builder:

1. Produces a block according to the L2 protocol rules
2. Computes the block content hash using the `ComputeBlockContentHash` function:

```solidity
function ComputeBlockContentHash(block, transactions) {
    // Create ordered list of all transaction hashes
    transactionHashes = []
    for each tx in transactions:
        txHash = keccak256(rlp_encode(tx))
        transactionHashes.append(txHash)
    
    // Compute a single hash over block data and transaction hashes
    // This ensures the hash covers the exact transaction set and order
    return keccak256(abi.encode(
        block.parentHash,
        block.number,
        block.timestamp,
        transactionHashes
    ))
}
```

3. Computes `blockContentHash = ComputeBlockContentHash(block, block.transactions)`

This block content hash formulation provides a balance between rollup compatibility and verification strength:
- Contains the minimal set of elements needed to uniquely identify a block's contents
- Compatible with data available on L1 for most optimistic and ZK rollup implementations
- Enables signature verification without requiring state root dependencies
- Supports future L1 verification of block authenticity across different rollup designs

### Verification Contract

The smart contract that verifies block builder TEE proofs would implement:

```solidity
// Block builder verification contract
function verifyBlockBuilderProof(bytes32 blockContentHash) external {
    // Check if the caller is an authorized TEE block builder
    require(
        IAllowlist(ALLOWLIST_CONTRACT).isAllowedPolicy(BLOCK_BUILDER_POLICY_ID, msg.sender),
        "Unauthorized block builder"
    );
    
    // At this point, we know:
    // 1. The caller is a registered address from an attested TEE
    // 2. The TEE is running an approved block builder workload (via policy)
    
    // Additional validation can be performed on the blockContentHash
    
    emit BlockBuilderProofVerified(
        msg.sender,
        block.number,
        blockContentHash
    );
}
```

### Security Properties

This mechanism provides several important security guarantees:

1. **Block Authenticity**: Each block contains cryptographic proof that it was produced by an authorized TEE block builder
2. **Non-Transferability**: The proof cannot be stolen or reused by unauthorized parties due to TEE protection of signing keys
3. **Policy Flexibility**: The system can adapt to new block builder implementations by updating the policy without contract changes
4. **Auditability**: All proofs are recorded onchain for transparency and verification

### Integration with Rollup Systems

For rollup systems, this proof mechanism:

1. Can be included as the final transaction in each block
2. Enables L1 contracts to verify that blocks were built by authorized TEEs
3. Provides a foundation for stronger security guarantees in optimistic rollups
4. Supports cross-chain verification of block authenticity
