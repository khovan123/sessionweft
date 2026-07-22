# Security GA Review — SessionWeft 0.1.0

Decision: **Approved for General Availability within the declared scope**.  
Approving authority: `khovan123`  
Analysis executor: `sessionweft-automation`  
Review date: 2026-07-22

## Review basis

External reference baseline:

- NIST Secure Software Development Framework 1.1: https://csrc.nist.gov/pubs/sp/800/218/final
- OWASP Application Security Verification Standard 5.0.0: https://owasp.org/www-project-application-security-verification-standard/
- SLSA Build track 1.2: https://slsa.dev/spec/v1.2/build-track-basics
- GitHub artifact attestations and SBOM attestations: https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations
- NIST Incident Response SP 800-61 Rev. 3: https://csrc.nist.gov/pubs/sp/800/61/r3/final

## Security evidence reviewed

- Default-deny Tool and MCP authorization.
- Scoped, expiring and one-time approval consumption before external side effects.
- Runtime validation of Session, Agent, capability, risk, lock lease and fencing token.
- No-shell process execution, executable allowlists, canonical working directories and environment allowlists.
- Linux bubblewrap isolation with network denied by default and explicit filesystem bindings.
- Malicious plugin tests for initialization hangs, duplicate tools, schema spoofing, output flooding and secret probing.
- Canonical workspace and symlink escape protections.
- Durable execution ledger that prevents blind retries of uncertain external side effects.
- Constant-time bearer token comparison and authenticated control-plane endpoints.
- RustSec audit, source/license policy, tracked-secret scan and fail-closed SQLx MySQL compatibility boundary.
- CycloneDX SBOM, checksums and GitHub/Sigstore provenance attestations.
- Threat model, alerts, incident runbook and recovery procedures.

## Findings

- Open Critical findings: 0.
- Open High findings: 0.
- No tested control allows stale lock ownership, duplicate durable side effects, workspace escape or plugin access to non-allowlisted secrets.
- Build provenance and SBOM generation are release blocking.
- Artifact attestations prove build identity and provenance, not the absence of vulnerabilities; vulnerability and control testing remain independently required.
- The production sandbox claim applies to the Linux bubblewrap profile only.

## Residual security risks

- Model and provider output remains untrusted input and must continue through policy, approval and bounded execution paths.
- New plugins and providers require their own compatibility and malicious-behavior tests.
- Operators remain responsible for TLS termination, secret-manager configuration, host hardening and network policy in their deployment environment.
- Non-Linux plugin execution is not approved as a production sandbox under this GA decision.
- Security advisories published after the tested commit can invalidate release suitability and require incident/release response.

## Approval

Security is approved for SessionWeft 0.1.0 GA within the scope recorded in `ga-authorization.md`, contingent on all release gates remaining passed for the exact tested commit and Critical/High findings remaining zero.
