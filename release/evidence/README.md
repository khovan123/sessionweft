# Release Evidence

`rc-0.1.0.json` is a committed template, not proof that a release was tested.

The production-hardening and release workflows must materialize `verified.json` for the exact `GITHUB_SHA`, evaluate it with `sessionweft-release-gate`, and publish it together with checksums and the CycloneDX SBOM.

Rules:

- a committed template must use `commit: TBD`;
- an RC artifact must reference the exact tested hexadecimal commit;
- automated sign-offs may authorize an RC only;
- General Availability requires named human Architecture, Security and Operations sign-offs;
- release packages built without verified evidence are invalid.
