# Contributing

- **License and CLA.** Read [LICENSING.md](LICENSING.md). Every pull request needs the author's agreement to the [CLA](CLA.md), given as a comment on the pull request (CLA section 7). Only the rights holder can agree; an agent or bot cannot do it on anyone's behalf.
  - **Why a CLA.** Café Space is offered under the AGPL and under commercial licenses, and the commercial licenses fund its development. Offering both needs a license to every contribution, so the CLA grants one. You keep your copyright.
  - **Our promise.** Café Space stays open source. Every version that contains your contributions remains available under an OSI-approved license (CLA section 4). A commercial license is only ever an extra option and never replaces the open-source one.
- **Public repository.** Never commit private links, real names or emails of other people, room or member IDs, invitations, tokens, room message content, local paths or secrets. Use placeholders in examples.
- **Commit identity.** Use your GitHub noreply address for commits if you do not want your email in a public history.
- **Before pushing.** Pull with `--ff-only`, run the checks in [README.md](README.md), and never force-push `main`.
- **Design first.** Changes that alter decisions or architecture go into [docs/architecture.md](docs/architecture.md) in the same change. Protocol changes come with fixtures first.
