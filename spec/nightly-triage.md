# Nightly pull request triage

Be concise and avoid unnecessary notifications and comments. Follow the repository's agent instructions and use the current default branch as the basis for triage.

## 1. Dependabot

Merge straightforward Dependabot patch or minor updates only when CI is green and the changes are unproblematic. Leave breaking, uncertain, or failing updates for human review.

## 2. Other people's pull requests

Review pull requests opened by others, excluding Dependabot and your own, only when a genuinely new finding can help. Skip drafts, unchanged heads on which you already left findings or requested changes, and pull requests that are only conflicting because the base advanced.

Maintain exactly one automated issue comment per engaged pull request. List comments using `gh api repos/<owner>/<repo>/issues/<number>/comments` and match the raw body against `<!-- claude-nightly-review -->`; the marker is invisible in rendered views. If multiple marked comments exist, retain the oldest and delete the others. If the surviving note already records the current head SHA, do not edit it. Otherwise replace its body with current findings, or create one only when there is a genuinely new finding. Start it with the marker followed by `Automated nightly note for @<me> — not a merge decision. Reviewed <short-sha> on <date>.` Inspect files at the actual PR head. Never submit a formal review or change unmarked human comments.

## 3. Your own pull requests

Never merge your own PR. Leave it for the maintainer. At most one Claude-authored PR may be awaiting review; if one exists, do not open another, but continue useful PR reviews.

## 4. Explicitly assigned fallback work

If there are no mergeable simple Dependabot PRs and no unmerged Claude PR awaiting review, read [the autonomous queue](roadmap.md#autonomous-nightly-agent-queue). Select its first unstarted item. If it is empty, an unchecked lead in [the bug audit](roadmap.md#bug-audit---2026-08-28-ongoing) is also explicitly eligible. Follow the roadmap's instructions for investigating and recording it. Open at most one PR for review.

Other roadmap items are not assignments, even if they are unfinished or appear achievable. Do not invent a task, re-audit a cleared lead without new evidence, or choose unlisted work. If neither eligible list has work, report the empty queue rather than creating noise.
