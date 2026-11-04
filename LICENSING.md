# Licensing

Decision D16 in the [architecture](docs/architecture.md), 2026-11-04.

| Part | License |
|---|---|
| **The core**: daemon, CLI, desktop shell, web UI, cloud and everything else not listed below | **GNU Affero General Public License v3.0** (AGPL-3.0-only), with a **commercial license** from the Project's owner for anyone who cannot meet the AGPL's terms. Full text: [LICENSE](LICENSE) |
| **The protocol**: specs in `docs/protocol/`, types and codecs in `crates/protocol/`, examples in `fixtures/v2/`, generated bindings in `web/src/protocol/generated/`, and the future room-protocol spec and SDKs | **Apache License 2.0**, so that any agent or tool can implement the protocol. Full text: [LICENSE-APACHE](LICENSE-APACHE). `crates/protocol` declares `Apache-2.0` in its manifest |
| **Derived from v0**: `fixtures/v0-go/` | **MIT**, copyright cafecodework. The original notice is kept in [fixtures/v0-go/LICENSE-MIT](fixtures/v0-go/LICENSE-MIT) |

- **Contributions.** Every contribution needs the [CLA](CLA.md). It grants the Project's owner (GitHub `shirtiny`) the rights needed to offer the commercial license. You keep your copyright, and the owner commits to keeping the Project available under an OSI-approved license.
- **Commercial license.** Sold by the Project's owner as an individual. Contact <shirtiny@gmail.com>.
- **Names and marks.** "Café Space", "Café" and the cup mark are trademarks of the Project's owner. The licenses above cover the code, not the names or marks. A fork must use its own name.
- **Status.** In effect from 2026-11-04. All contributions up to that date are owned by the repository owner, who licensed them as above ([docs/legal/cla-signatures.md](docs/legal/cla-signatures.md)). Later contributions from anyone else need the CLA.
