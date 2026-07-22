# Installation and Upgrade Guide

## Verify a release bundle

A release candidate bundle is valid only when it contains:

- `sessionweftd`
- `sessionweft` CLI
- `sessionweft-tui`
- SHA-256 checksum file
- CycloneDX SBOM
- GitHub build-provenance and SBOM attestations

Verify checksums before extracting:

```bash
sha256sum --check sessionweft-<version>-linux-x86_64.sha256
```

Verify GitHub attestations with the GitHub CLI:

```bash
gh attestation verify sessionweft-<version>-linux-x86_64.tar.gz \
  --repo khovan123/sessionweft
```

Do not install an artifact whose checksum, SBOM or provenance cannot be verified.

## Local SQLite installation

1. Extract the verified bundle into a directory owned by the Runtime service account.
2. Place the binaries on a restricted executable path.
3. Configure:

```text
SESSIONWEFT_BIND=127.0.0.1:7447
SESSIONWEFT_DATABASE_URL=sqlite:///var/lib/sessionweft/sessionweft.db
SESSIONWEFT_WORKSPACE_ROOT=/srv/sessionweft/workspaces
SESSIONWEFT_API_TOKEN=<secret-manager-reference>
```

4. Start `sessionweftd` under a supervisor.
5. Verify `/health/live`, authenticated `/health/ready` and the client protocol.
6. Back up the SQLite database before every upgrade.

Local mode is not highly available and must not be presented as service-mode HA.

## PostgreSQL/JetStream service mode

Use `deploy/docker-compose.service.yml` only as a development/reference deployment. Production operators must provide durable volumes, TLS, secret management, network policy, backup automation and monitored PostgreSQL/NATS services.

Required configuration:

```text
SESSIONWEFT_DATABASE_URL=postgres://<user>:<secret>@<host>/<database>
SESSIONWEFT_NATS_URL=nats://<host>:4222
SESSIONWEFT_RUNTIME_INSTANCE_ID=<unique-stable-id>
SESSIONWEFT_API_TOKEN=<secret-manager-reference>
```

Every concurrent Runtime and worker instance needs a unique stable instance ID.

## Upgrade

1. Verify the new artifact and release evidence.
2. Complete PostgreSQL, JetStream and local-state backups.
3. Read release notes and compatibility requirements.
4. Follow `docs/09-operations/rolling-upgrade.md`.
5. Observe one full retry/redelivery window after each instance.
6. Keep the previous attested artifact available until the rollout is accepted.

## Uninstall

Stop Runtime and worker processes before removing binaries. Preserve Session databases, workspaces, Git repositories, backups, audit events and release evidence according to retention policy. Removing binaries must not imply deleting durable user state.
