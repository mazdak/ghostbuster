# Security policy

## Supported versions

Ghostbuster is pre-1.0. Security fixes are applied to the latest release only.

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Use GitHub's **Report a
vulnerability** button in the repository Security tab to submit a private security advisory.
Include affected versions, impact, reproduction steps, and any suggested mitigation.

You should receive an acknowledgement within seven days. We will coordinate disclosure after a fix
is available and credit reporters who want to be named.

Ghostbuster analyzes untrusted Python as data and does not execute it. Reports involving parser
crashes, resource exhaustion, path traversal, release provenance, or unsafe handling of source
files are in scope.
