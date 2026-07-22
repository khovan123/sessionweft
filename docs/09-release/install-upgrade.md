# Install and Upgrade SessionWeft

## Verify a release

A valid Release Candidate contains:

- `sessionweft-<version>-<target>.tar.gz`;
- matching `.sha256` file;
- CycloneDX SBOM;
- GitHub provenance attestation;
- `release-policy.json` and `release-evidence.json` inside the archive.

Verify the archive before extraction:

```bash
sha256sum --check sessionweft-0.1.0-rc.1-<target>.tar.gz.sha256
```

Verify the GitHub attestation with the GitHub CLI:

```bash
gh attestation verify sessionweft-0.1.0-rc.1-<target>.tar.gz \
  --repo khovan123/sessionweft
```

## Install local mode

1. Extract the archive into a versioned directory.
2. Put the required binaries from `bin/` on `PATH`.
3. Create a writable Runtime data directory.
4. Bind Runtime to loopback unless a bearer token is configured.
5. Start `sessionweftd`, then attach with the CLI or TUI.

Example:

```bash
tar -xzf sessionweft-0.1.0-rc.1-<target>.tar.gz
export PATH="$PWD/sessionweft-0.1.0-rc.1-<target>/bin:$PATH"
export SESSIONWEFT_DATABASE_URL="sqlite:$HOME/.local/share/sessionweft/runtime.db"
export SESSIONWEFT_BIND="127.0.0.1:7447"
sessionweftd
```

Local committed state is stored in SQLite WAL. Back up the database through a consistent SQLite backup mechanism rather than copying only the main database file while Runtime is writing.

## Install service mode

Use `deploy/docker-compose.service.yml` as a development/reference stack. Production deployments must provide:

- managed PostgreSQL or an equivalent backed-up cluster;
- NATS JetStream with persistent storage;
- unique stable Runtime instance IDs;
- secrets from the deployment secret manager;
- TLS/network policy appropriate to the environment;
- Prometheus scraping of exported operations metrics;
- verified PostgreSQL and JetStream backup procedures.

Never commit production passwords in Compose or `.env` files.

## VS Code extension

The repository release pipeline type-checks the extension. Package it using the VS Code extension tooling approved by the deployment environment, then verify the package digest beside the Runtime artifacts. Runtime bearer tokens belong only in VS Code SecretStorage.

## Upgrade

Follow `docs/10-deployment/upgrade-and-rollback.md`. Upgrade Runtime and workers one instance at a time, confirm readiness and observe an event retry window before continuing.

## Release gate

Validate included evidence:

```bash
sessionweft-release-gate \
  --policy config/release-policy.json \
  --evidence config/release-evidence.json \
  --level rc
```

`--level ga` intentionally fails for automated RC evidence. General Availability requires human architecture, security and operations sign-offs.
