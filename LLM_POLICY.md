## LLM policy

You may use LLMs, but you can't let them do the whole job.
LLMs are fine for boilerplate, explaining an unfamiliar API, debugging help, or a rough first draft that you then rewrite.
They are not fine for generating an entire PR (code, tests, or description) and submitting it as is.

Rules:

- **Review everything.** Read every line the LLM produced before opening a PR. Build it, run it, and test it yourself. Remove anything you don't understand.
- **Disclose it.** Say in the PR which LLM you used and what for (the PR template has a spot for this). If you didn't use one, say that instead. Being upfront is never held against you.
- **You must be able to explain your changes.** If I ask why something works the way it does, "the LLM wrote it" is not an answer.
- **Write your own PR descriptions, issues and comments.** They can be short and a bit messy, I'd rather read that than polished generated text. Using an LLM to *translate* your own words is fine, but include the original text too.
- **Keep PRs small and focused.** Large generated diffs, mass renames and drive-by "cleanups" will be closed.

This policy is motivated by several observations:

- LLM writing is long and padded, and wastes the reader's time.
- LLM analysis of issues is frequently wrong, and sounds confident while being wrong.
- Unreviewed LLM code often calls functions, crates or flags that don't exist, or quietly doesn't build.
- LLM-assisted PRs are usually a bigger chore to review, and the author often struggles to address review comments because they don't fully understand the code.
- LLMs remove the *effort barrier* for writing code. The result reads fine but is often overly complex, solves problems nobody has, and adds maintenance burden for no real gain.
    This applies to tests too: generated tests tend to be verbose, repeat themselves, and test trivially true things instead of actual outcomes.
- I want contributors who understand drift's codebase, and that only happens if you do the work and read the code yourself.
- Nobody wants to read AI slop.

PRs that don't build, were clearly never run, contain undisclosed LLM use, or that the author can't explain will be closed.
