# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| Latest release (0.1.x) | ✅ |
| Older versions | ❌ (please upgrade) |

## Reporting a vulnerability

Please **do not** disclose security issues in public issues. Prefer GitHub's private vulnerability reporting:

1. Open the repository's **Security → Advisories → Report a vulnerability**
   (`https://github.com/TrapMeHere/NEO/security/advisories/new`);
2. Or email the maintainers (see their GitHub profiles), with the subject prefix `[NEO Security]`.

Please include: affected version, reproduction steps, impact (data loss/arbitrary code execution/crash, etc.),
a PoC if available, and your contact details.

## Response process

- Acknowledge receipt within 48 hours;
- Provide an initial assessment and fix plan within 7 days;
- After the fix, publish a patch release and publicly credit you in the GitHub Security Advisory (if you agree).

## Scope

- PBF/XML/osmChange parsing in `neo-io` (malicious files causing crashes/OOM/out-of-bounds)
- Journal recovery and session file (.jos/.joz) parsing in `neo-editor`
- `neo-render`/wgpu integration, font/icon resource loading
- Dependency supply chain (`cargo deny`/Dependabot)

## Out of scope

- Issues with the upstream OSM API/data services
- NVIDIA Overlay/driver pop-ups (see FAQ)
- Conflicts with third-party software on the user's machine
