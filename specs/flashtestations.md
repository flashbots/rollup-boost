 Flashtestations: Transparent Onchain TEE Verification and Curated Allowlist Protocol

*Authors: [fnerdman](https://github.com/fnerdman), [Melville](https://github.com/Melvillian), [dmarz](https://github.com/dmarzzz), [Ruteri](https://github.com/Ruteri)*

**Table of Contents**
- [Abstract](#abstract)
- [Prerequisites](#prerequisites)
- [Motivation](#motivation)
- [Specification](#specification)
  - [Terminology](#terminology)
    - [Intel TDX Primitives](#intel-tdx-primitives)
    - [Flashtestations Protocol Components](#flashtestations-protocol-components)
    - [Operational Terms](#operational-terms)
  - [Data Structures](#data-structures)
    - [**`TDXQuote`**](#tdxquote)
    - [**`TDReport`**](#tdreport)
    - [**`DCAPEndorsements`**](#dcapendorsements)
    - [**`TDXMeasurements`**](#tdxmeasurements)
  - [System Architecture](#system-architecture)
  - [TEE Attestation Mechanism](#tee-attestation-mechanism)
    - [Intel TDX DCAP Attestation](#intel-tdx-dcap-attestation)
    - [Onchain DCAP Attestation](#onchain-dcap-attestation)
    - [Workload Identity Derivation](#workload-identity-derivation)
  - [Allowlist Registry](#allowlist-registry)
    - [Core Concepts](#core-concepts)
    - [Key Relationship Model](#key-relationship-model)
    - [Fundamental Operations](#fundamental-operations)
    - [Key Requirements](#key-requirements)
    - [Attestation Verification Endpoint](#attestation-verification-endpoint)
  - [Policy Layer: Flexible Authorization](#policy-layer-flexible-authorization)
    - [Policy Abstraction](#policy-abstraction)
    - [Policy Operations](#policy-operations)
  - [End-to-End Flow](#end-to-end-flow)
    - [Attestation and Registration](#attestation-and-registration)
    - [Runtime Authorization](#runtime-authorization)
    - [Maintenance: Handling Changing Endorsements](#maintenance-handling-changing-endorsements)
    - [Gas Cost Considerations and Future Optimizations](#gas-cost-considerations-and-future-optimizations)
  - [Offchain TEE Address Verification](#offchain-tee-address-verification)
    - [Example Verification Flow](#example-verification-flow)
  - [Transparency Log](#transparency-log)
    - [Purpose and Benefits](#purpose-and-benefits)
    - [Logged Information](#logged-information)
    - [Implementation Approach](#implementation-approach)
    - [Relationship with Allowlist](#relationship-with-allowlist)
- [Rationale](#rationale)
  - [Replacement Model](#replacement-model)
  - [Gas Optimization](#gas-optimization)
  - [Separation of Concerns](#separation-of-concerns)

# Abstract

Trusted Execution Environments (TEEs) offer a promising approach for running confidential workloads with hardware-enforced security guarantees. However, integrating TEEs with blockchain applications presents significant challenges: How can smart contracts verify that they're interacting with authentic TEE services running expected code? How can this verification scale efficiently onchain? How can we maintain an up-to-date registry of validated services as hardware security requirements evolve?

Flashtestations addresses these challenges by providing a comprehensive onchain protocol for TEE verification, address registration, and transparent record-keeping. The protocol enables:

1. Onchain verification of Intel TDX attestations against current Intel endorsements
2. Maintenance of a curated allowlist of validated Ethereum addresses associated with specific TEE workloads
3. Policy-based authorization for TEE services to securely interact with smart contracts
4. Transparent logging of all attestation events and endorsement changes

# Prerequisites

This document assumes familiarity with the following background material, specifications, and tooling. Items are arranged in the rough order they become relevant while reading this spec:

1. **Intel TDX Architecture & Security Model** — core concepts, measurement registers, Trust Domain isolation, and attestation flows.
   • *Key reference:* [Intel TDX Specifications and Developer Guides](https://www.intel.com/content/www/us/en/developer/tools/trust-domain-extensions/documentation.html)
2. **Intel DCAP Attestation Stack** — Quote generation, signature scheme and collateral (QE Identity & TCB Info) retrieval.
   – [Intel TDX DCAP Quoting Library API](https://download.01.org/intel-sgx/latest/dcap-latest/linux/docs/Intel_TDX_DCAP_Quoting_Library_API.pdf)
3. **On‑Chain DCAP Quote Verification** — Solidity contracts that decode DCAP quotes and perform cryptographic validation using PCCS‑sourced endorsements.
   • [Automata DCAP Attestation Contract](https://github.com/automata-network/automata-dcap-attestation)
4. **On‑Chain Endorsement Storage (PCCS)** — Automata’s Solidity implementation that mirrors Intel collateral on Ethereum, enabling fully reproducible verification.
   • [Automata On‑chain PCCS](https://github.com/automata-network/automata-on-chain-pccs)

# Motivation

Flashtestations is designed to achieve the following objectives:

1. **Security**: Provide cryptographic proof that a service is running inside a genuine TEE with expected code, with verification resistant to spoofing or replay attacks.

2. **Efficiency**: Ensure key operations (especially allowlist lookups) are gas-efficient enough for regular use in smart contracts, with O(1) gas costs regardless of allowlist size.

3. **Maintainability**: Support efficient updates as Intel endorsements evolve, without requiring re-verification of all attestations.

4. **Flexibility**: Enable policy-based access control that can adapt to different trust requirements without modifying consumer contracts.

5. **Transparency**: Maintain auditable records of all attestations and endorsement changes to support accountability and security analysis.

6. **Separation of Concerns**: Clearly separate allowlist mechanics from policy governance, enabling independent evolution of these components.

# Specification

## System Architecture

Within the Flashtestations specification, the protocol architecture consists of four key components that work together to provide secure onchain TEE verification:

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
│                     │                  │ │                 │ │
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

**`Endorsement Version`**: The specific version of Intel DCAP endorsements at a point in time. Endorsements change periodically as Intel releases updates or discovers vulnerabilities in hardware or firmware.

**Allowlist Registry**: The onchain data structure that tracks which Ethereum addresses have been validated for specific workloads based on successful attestation. Implemented as the TdxAllowlist contract.

**Policy Registry**: A mapping system that groups related workload identities under a single policy identifier, enabling flexible authorization rules without modifying consumer contracts.

**Transparency Log**: The onchain event-based system that records all attestation verifications, allowlist changes, and endorsement updates for auditability. Implemented through emitted blockchain events rather than as a separate logging service.

**Onchain Verifier**: The smart contract component (using Automata's DCAP attestation system) that validates TDX attestation quotes against current Intel DCAP endorsements and interacts with the Allowlist Registry to register validated addresses.

**Workload**: The specific software running inside a TEE. Its identity is derived from measurement registers that contain cryptographic hashes of loaded code and configuration.

**`policyId`**: An identifier that maps to a list of approved `workloadId`s, enabling contracts to reference policies rather than specific workloads.

**PCCS (Provisioning Certificate Caching Service)**: Automata's onchain implementation of Intel's PCCS that stores Intel DCAP endorsements on the blockchain, making it available for attestation verification. This ensures all verification is reproducible on L2.

### Operational Terms

**Registration**: The process of adding an Ethereum address to the allowlist after successful attestation verification.

**Endorsement Revocation**: The process of marking attestations as outdated when Intel updates its security requirements.

**Housekeeping**: The maintenance process of verifying and updating attestation status when endorsements change. This can be done on-demand via the attestation verification endpoint.

**TCB Recovery**: The process that occurs when Intel releases updates to address security vulnerabilities in the TCB components. This typically requires updating the list of secure endorsements.

**Reproducible Build**: A deterministic build process that ensures anyone building the same source code will produce identical binary outputs, enabling verification of expected TEE measurements.

## Data Structures

The protocol defines several key data structures:

### **`TDXQuote`**

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

### **`TDReport`**

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
- `MROWNER`: Measurement register that takes arbitrary information and can be set by the infrastructure operator during before startup of the VM
- `MRCONFIGID`: same as `MROWNER`
- `MROWNER_CONFIG`: same as `MROWNER`
- `ReportData`: Confidential-VM defined data included in the report (e.g., public key hash).

### **`DCAPEndorsements`**

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

### **`TDXMeasurements`**

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
- `MROWNER`: Contains the infrastructure operator's public key (Ethereum address or other identifier).
- `MRCONFIGID`: Hash of service configuration stored onchain and fetched on boot.
- `MROWNERCONFIG`: Contains unique instance ID chosen by the operator.

## TEE Attestation Mechanism

Attestation is the process by which a TEE proves its identity and integrity. This section of the specification defines how the protocol uses Intel TDX with DCAP (Data Center Attestation Primitives) attestation.

### Intel TDX DCAP Attestation

TDX attestation produces a Quote structure as defined in the [TDXQuote](#tdxquote) and [TDReport](#tdreport) sections.

The attestation process follows these steps:

1. The TEE generates a TD Report containing its measurement registers and report data
2. The Quote Enclave (QE) creates a Quote by signing the TD Report with an Attestation Key
3. The Quote can be verified against Intel's Provisioning Certification Service (PCS)

### Onchain DCAP Attestation

The following code sample illustrates how DCAP attestation verification is performed onchain, and how the key components (workloadId, quote, and Ethereum address) are extracted and registered in the Flashtestations allowlist:

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
    
    // Register the address in the allowlist with the raw quote for future verification
    IAllowlist(ALLOWLIST_CONTRACT).addAddress(workloadId, ethAddress, rawQuote);
    
    emit TEEServiceRegistered(workloadId, ethAddress, rawQuote);
}
```

This implementation highlights several key aspects:
1. The DCAP attestation is verified using Automata's onchain verifier
2. The workloadId is derived from the quote's measurement registers
3. The Ethereum address is extracted from the quote's report data
4. The extracted information and raw quote are registered in the allowlist

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

**Note on Reproducible Builds**: To establish trust in expected measurements, TEE workloads must use reproducible build processes where source code, build environment, and instructions are published, allowing independent verification that expected measurements correspond to the published source code.

## Allowlist Registry

The Allowlist Registry is a core component of Flashtestations that acts as a bookkeeper for tracking which Ethereum addresses have successfully passed attestation within a Trusted Execution Environment (TEE).

### Core Concepts

At its most abstract level within this specification, the Allowlist Registry is responsible for:

1. **Storing addresses** that have been validated through attestation
2. **Associating addresses** with their specific workload identity
3. **Storing attestation quotes** for future verification and revocation
4. **Providing efficient lookup** capabilities to verify if an address is authorized for a particular workloadID

The registry operates on these key abstractions:

1. **Workload Identity (`workloadId`)**: A 32-byte hash derived from TDX measurement registers (as defined in [Workload Identity Derivation](#workload-identity-derivation)) that uniquely identifies a specific piece of code running in a TDX environment. This serves as the primary namespace under which addresses are stored.

2. **Attestation Quote**: The raw attestation data provided during registration that contains the cryptographic proof of the TEE's state. This quote is stored alongside the address for later verification and revocation.

3. **Ethereum Address**: The public key extracted from the attestation's report data field ([TDReport.ReportData](#tdreport)), which will be used to interact with onchain contracts.

### Key Relationship Model

The Allowlist Registry maintains a straightforward relationship between these entities:

1. Each Ethereum address can be registered for multiple different workloads
2. Each address has exactly one attestation quote stored for each workloadId it's registered under
3. The registry tracks whether each (address, workloadId) pair is currently valid or has been marked as outdated

### Fundamental Operations

The Allowlist Registry provides these core operations:

#### 1. Lookup

The most frequent operation is checking if an address is valid for a specific workload:

```
function isAllowedWorkload(workloadId, address) → boolean
```

This simply checks if the address is currently associated with the specified workload and has not been marked as outdated. This operation must be highly gas-efficient as it may run on every block.

#### 2. Registration

When an attestation successfully passes verification:

```
function addAddress(workloadId, address, quote)
```

This operation:
1. Records that this address has been validated for this workload
2. Stores the raw attestation quote for future reference and verification
3. If the address was previously registered for this workloadId, the old entry is replaced

#### 3. Attestation Verification

To verify if an attestation is still valid against current endorsements:

```
function verifyAttestation(address) → boolean
```

This operation:
1. Retrieves the stored attestation quote for the address
2. Verifies it against current Intel endorsements
3. If verification fails, marks the address as outdated in the registry
4. Returns the verification result

#### 4. Quote Retrieval

To retrieve the stored attestation quote for a specific address:

```
function getQuoteForAddress(address) → bytes
```

This operation returns the raw attestation quote that was used to register the address, enabling offline verification or additional analysis of the attestation data.

### Key Requirements

1. **Simple Storage Model**: The registry maintains a simple mapping between addresses, workloadIds, and their attestation quotes without tracking complex endorsement relationships.

2. **Individual Verification**: Instead of batch removal based on endorsement bundles, attestations are verified individually when needed, allowing for more granular management.

3. **Quote Storage**: The system maintains a copy of the attestation quote used to register each address, supporting external verification and auditability.

4. **Gas Efficiency**: The lookup operation must be extremely efficient (O(1) gas costs) regardless of the number of addresses stored.

### Attestation Verification Endpoint

The attestation verification endpoint provides a mechanism to validate stored attestations against current Intel endorsements:

1. **On-demand Verification**: Verification happens only when needed, rather than requiring constant maintenance.

2. **Simple Management**: When Intel updates its endorsements, the system automatically adapts during verification.

3. **Smooth Transitions**: Addresses are marked as outdated only when their verification actually fails.

The verification process works as follows:

1. The endpoint accepts an address as input
2. It retrieves the stored attestation quote for that address
3. It runs the verification against current Intel endorsements
4. If verification fails, it marks the address as outdated in the registry
5. The address remains in the registry but will fail the `isAllowedWorkload` check

This approach provides a clean, straightforward way to manage attestation validity over time.

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
   allowlist.addAddress(derivedWorkloadId, extractedAddress, rawQuote)
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

2. This policy check determines if the address is allowed for any workload in the policy and has not been marked as outdated.

### Maintenance: Handling Changing Endorsements

Intel endorsements change over time, requiring a maintenance process:

1. **Passive Verification**: When addresses are verified using the `verifyAttestation` function, the system checks if their attestation is still valid against current endorsements.

2. **Marking as Outdated**: If verification fails due to outdated endorsements, the address is automatically marked as outdated.

3. **Re-attestation**: Addresses marked as outdated must re-attest using current endorsements to regain valid status.

This approach ensures that addresses naturally transition from valid to outdated as Intel's security requirements evolve, without requiring manual tracking of endorsement changes or complex batch operations.

The maintenance process keeps the allowlist in sync with Intel's current security opinions while allowing for graceful transitions when endorsements change.

### Gas Cost Considerations and Future Optimizations

The individual attestation verification approach prioritizes simplicity but may incur higher gas costs compared to bulk operations. Each verification requires running the complete attestation verification process against current endorsements.

Future optimizations could include:

1. **Tracking Endorsement References**: Store a reference to which endorsement version validated each attestation. When endorsements become outdated, all attestations linked to that specific endorsement could be marked invalid in a single operation.

2. **Validation on Access**: Alternatively, the system could verify the endorsement status upon each call to `isValidWorkload`, checking if the original validating endorsement is still considered secure without re-running the full attestation verification.

These optimizations would maintain the design's simplicity while providing more gas-efficient ways to handle endorsement changes, especially as the number of registered addresses grows.

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

As specified in this protocol, the transparency log captures raw attestation data along with verification results:

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
    address ethAddress,
    bool success
);

event EndorsementUpdated(
    bytes rawEndorsementData,
    bool isValid
);

event AllowlistUpdated(
    bytes32 indexed workloadId,
    address indexed ethAddress,
    bool isAdded,
    bool isValid
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

This dual approach specified in the protocol enables efficient onchain operations while maintaining complete transparency and auditability.

# Rationale

The following explains the reasoning behind key design decisions in the Flashtestations protocol:

### Replacement Model

The protocol uses a direct replacement approach for attestations:

- When an address re-attests for a workloadId, its old entry is replaced with the new one
- This keeps the model simpler by ensuring each address has exactly one current endorsement per workloadId
- When endorsements become invalid, all addresses using that specific endorsement are removed completely

### Gas Optimization

The rationale for gas optimization in the protocol design is that the system must prioritize efficiency, particularly for the lookup operations:

- Lookups should reflect O(1) gas costs regardless of the number of addresses
- Storage slots should be fully cleared when no longer needed (to receive gas refunds)
- Batch operations should be supported for removing addresses when endorsements become invalid

### Separation of Concerns

A key design rationale is maintaining clear separation between:

1. **Allowlist Registry**: Tracks which addresses have attestations validated by current endorsements
2. **Policy Registry**: Defines which workloads are acceptable for specific onchain operations
3. **Verification Service**: Validates attestations and updates the allowlist
4. **Consumer Contracts**: Use policy checks to authorize operations

This separation enables each component to evolve independently, with governance focused on the appropriate level of abstraction.

The Allowlist Registry also provides direct access to stored attestation quotes, allowing external systems to perform their own verification or analysis without requiring additional onchain transactions.

