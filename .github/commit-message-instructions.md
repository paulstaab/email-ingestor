# Commit message generation

Generate a single commit message using Conventional Commits:

```text
<type>(<optional scope>): <short description>

<optional body explaining why the change is needed>

<optional footers>
```

- Base the message on the staged diff when available. If nothing is staged,
  describe only the supplied changes. Do not infer unrelated work from chat.
- Use `feat` for new behavior, `fix` for bug fixes, `docs` for documentation,
  `test` for tests, `refactor` for restructuring without behavior changes,
  `perf` for performance, `ci` for workflows, `build` for dependencies or build
  tooling, `style` for formatting, and `chore` for other maintenance.
- Add a short scope only when it helps, such as `smtp`, `imap`, `config`,
  `streaming`, `container`, or `copilot`. Omit the parentheses when there is no scope.
- Write the description in English, using an imperative verb in lowercase
  after the prefix, with no trailing period. Aim for a subject of at most
  72 characters, including the prefix.
- Summarize the main purpose of the whole change. Supporting tests and
  documentation belong with the feature or fix rather than determining its type.
- For a simple change, output only the subject. For a complex change, add a blank
  line and a concise body explaining the problem, resulting behavior, and any
  material compatibility impact. Wrap body lines at about 72 characters.
- Mark an actual breaking change with `!` before the colon and explain it in a
  `BREAKING CHANGE:` footer. Do not mark additive, backward-compatible options
  as breaking changes.
- Include issue references only when supplied. Do not invent issue numbers,
  test results, reviews, image digests, publication, or deployment claims.
- Never include credentials, AUTH payloads, message contents, or private addresses.
- Output the commit message alone, without Markdown fences, a preamble, emojis,
  or a generated-by attribution.

Examples:

```text
feat(smtp): add authenticated plaintext transport on loopback
fix(imap): retain pending work after reconnect
ci: smoke-test both SMTP transports before publication
docs: explain the shared-pod SMTP security boundary
chore(copilot): configure commit message generation
```
