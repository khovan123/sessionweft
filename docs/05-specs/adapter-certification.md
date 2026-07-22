# Production Adapter Certification

Status: mandatory for SessionWeft 0.2.0 and later.

## Scope

Every production Provider, Plugin, Deployment and Billing adapter has a versioned manifest and exact-commit certification. Adding code to the registry is not sufficient to make an adapter production-active.

## Manifest

A manifest identifies:

- stable adapter ID and version;
- adapter kind;
- production status;
- supported platforms;
- capabilities;
- source paths;
- required certification gates.

The manifest is canonicalized and SHA-256 bound to the certification.

## Required gates

- **Contract**: implements the Runtime-owned interface and normalized errors/results.
- **Compatibility**: supported platform/service versions and reconnect semantics are tested.
- **Security**: authentication, secret boundaries, permissions and malicious-input tests pass.
- **Recovery**: cancellation, retry, idempotency and uncertain side effects are covered.
- **Observability**: stable correlated metrics/events exist without secret leakage.
- **Supply chain**: locked dependencies, advisory, source/license, SBOM and provenance gates pass.

## Exact-commit evidence

CI materializes certification records for `GITHUB_SHA`. A record with `TBD`, a different manifest digest, missing source path, duplicate gate or empty evidence fails. Certification output is uploaded as an immutable workflow artifact and included in the 0.2.0 release evidence.

## Activation rule

Runtime configuration may reference only adapter IDs present in the verified certification set for the current release commit. Unknown, uncertified or differently versioned adapters are rejected before secrets, workspace paths or network capabilities are provided.

## Change rule

Any change to adapter source paths, capabilities, dependencies, supported platforms or production policy invalidates the prior manifest digest and requires a new certification. Future adapters cannot inherit another adapter's evidence.
