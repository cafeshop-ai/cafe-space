# Licensing

Decision D16 in the [architecture](docs/architecture.md), 2026-11-04.

| Part | License |
|---|---|
| **The core**: daemon, CLI, desktop shell, web UI, cloud and everything else not listed below | **GNU Affero General Public License v3.0** (AGPL-3.0-only), with a **commercial license** from cafeshop.ai for anyone who cannot meet the AGPL's terms. The `LICENSE` file is added once every existing contributor has agreed to the [CLA](CLA.md) |
| **The protocol**: specs in `docs/protocol/`, types and codecs in `crates/protocol/`, examples in `fixtures/v2/`, generated bindings in `web/src/protocol/generated/`, and the future room-protocol spec and SDKs | **Apache License 2.0**, so that any agent or tool can implement the protocol |
| **Derived from v0**: `fixtures/v0-go/` | **MIT**, copyright cafecodework. The original notice is kept in [fixtures/v0-go/LICENSE-MIT](fixtures/v0-go/LICENSE-MIT) |

- **Contributions.** Every contribution needs the [CLA](CLA.md). It grants cafeshop.ai the rights needed to offer the commercial license. You keep your copyright, and cafeshop.ai commits to keeping the Project available under an OSI-approved license.
- **Commercial license.** Contact cafeshop.ai through <https://www.cafeshop.ai>.
- **Names and marks.** "Café Space", "Café" and the cup mark are trademarks of cafeshop.ai. The licenses above cover the code, not the names or marks. A fork must use its own name.
- **Status.** The core's `LICENSE` file is not in the repository yet, and existing contributions are not yet licensed under it, until the CLA agreements in [docs/legal/cla-signatures.md](docs/legal/cla-signatures.md) are complete. Development does not wait for this.
