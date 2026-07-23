# `sct gui` - clinical knowledge atlas

Status: Approved direction; implementation not started. Programme roadmap item: `R38`. Delivery stages: `GUI-1` through `GUI-8`.

## Decision summary

- `sct gui` will become a deliberately designed clinical terminology explorer, described internally as a **clinical knowledge atlas**. This is a product direction, not a rename of the command.
- The primary workflow is search -> understand a concept -> traverse its relationships. The interface opens directly into that work rather than a marketing or configuration screen.
- The visual design should make SNOMED CT feel like a coherent, living knowledge graph without becoming decorative, game-like, or clinically unserious.
- Concept identity, terminology meaning, hierarchy, defining attributes, mappings, history, and release provenance form one progressively disclosed workspace.
- Graph layouts must be stable and explanatory. Randomly moving force-directed clouds and unbounded whole-ontology visualisations are explicitly out of scope.
- The GUI remains a native localhost adapter over a user-supplied `snomed.db`. It is distinct from the future in-browser WebAssembly demo in `R3`, although the two may later share presentation code and interaction contracts.
- One canonical Rust engine drives CLI, SDK, MCP, FHIR, TUI, and GUI behaviour. GUI handlers should consume typed SDK query primitives rather than maintain independent terminology SQL.
- Runtime operation must make no external network requests. HTML, CSS, JavaScript, fonts, icons, and any graph library must be embedded or otherwise shipped with `sct` under compatible licences.
- The application shell uses the canonical `sct` logo from `docs/assets/logo/sct-logo.svg`. Do not redraw it or create a divergent GUI-specific mark.
- Retain the smallest frontend architecture that supports the product. Plain HTML, CSS, and JavaScript with no frontend build step remain the default; add a framework only after a measured need, not as a prerequisite for redesign.
- Playwright inspection at phone, tablet, and desktop sizes is part of implementation, not a final cosmetic review.

## Relationship to the current GUI

The existing `sct gui` is a useful experimental foundation rather than a failed product. `src/commands/gui.rs` already provides a read-only Axum server bound to localhost, `assets/index.html` provides an embedded single-page interface, and `--dev-html` supports refresh-without-recompile iteration. Search, concept detail, hierarchy summaries, children, size estimates, and one-hop graph data already exist.

The current implementation also identifies the main gaps this programme must address:

- DaisyUI, Tailwind, and D3 are loaded from public CDNs, which conflicts with the no-network-at-runtime invariant.
- The fixed sidebar layout does not recompose for phone or narrow tablet viewports.
- In-memory navigation does not integrate with URLs, refresh, browser back/forward, or shareable local links.
- Selecting a hierarchy searches for its name and filters those lexical results rather than performing genuine hierarchy navigation.
- The force-directed graph is useful as a demonstration but unstable as an explanatory model, silently limits children, and has no non-drag keyboard equivalent.
- The GUI adapters issue direct SQL and return untyped `serde_json::Value` envelopes instead of using the Rust SDK boundary.
- Concept detail does not yet present active status, release provenance, mappings, or history as first-class information.
- Loading, empty, error, focus, reduced-motion, zoom, and mobile states do not yet have durable browser coverage.

The redesign should evolve this implementation through working vertical slices. It should not begin with a speculative rewrite or a new frontend toolchain.

## Audience and jobs

The primary audience is a clinician, terminology specialist, researcher, developer, analyst, or informatician who has licensed SNOMED CT content locally and needs to inspect it quickly. The interface is a terminology browser, not diagnostic advice, clinical decision support, or a content-authoring environment.

The frequent jobs are:

1. Find the intended concept from a clinical phrase, synonym, or SCTID.
2. Confirm what a concept means from its preferred term, FSN, semantic tag, hierarchy, parents, and defining attributes.
3. Traverse to broader, narrower, or related concepts without losing context.
4. Understand where a concept maps to another terminology and whether an inactive concept has a current replacement.
5. Inspect which edition and release produced the answer.
6. Copy an SCTID, term, ECL expression, or local concept link into another workflow.
7. Explain a concept or relationship visually to another person.

Advanced jobs follow after the core explorer is strong: compare two concepts, inspect shared ancestry and differing definitions, compose or run ECL, and inspect larger bounded neighbourhoods.

## Product principles

### Atlas, not administration portal

The interface should feel like exploring structured clinical knowledge, not maintaining records in a generic enterprise dashboard. The subject must be visible in the first viewport. Search, concept identity, and topology are the composition; navigation chrome stays quiet.

### Meaning before identifiers

Preferred terms and logical relationships are primary. SCTIDs remain visible, selectable, copyable, and monospaced, but they do not dominate the visual hierarchy. Semantic tags should be separated visually from human-readable labels rather than left as noisy suffixes everywhere.

### Stable spatial context

Navigation should preserve a user's mental map. Parent concepts occupy a consistent direction, children another, and defining attributes another. Moving focus should animate only enough to explain the transition and must respect `prefers-reduced-motion`.

### Progressive disclosure

The first concept view answers "what is this?" immediately. Detailed identifiers, raw relationship fields, release metadata, mappings, history, and query tools remain easy to reach without crowding the frequent path.

### Local-first made visible

The selected edition and release should be present in the interface, together with a concise local/read-only indicator. This is useful provenance, not marketing copy. No terminology content, search query, filename, or interaction leaves the machine.

### Fast enough to feel direct

Typing, selection, traversal, and switching views should feel immediate on a warm local database. Loading indicators reserve stable space and appear only when work is perceptible. Performance regressions must be measured rather than hidden with animation.

## Visual direction

### Overall character

Use a restrained, high-contrast visual system: deep ink or graphite structure in dark mode, warm neutral surfaces in light mode, and a small set of purposeful electric accents. The tone is precise, contemporary, and clinical without defaulting to generic "medical teal." It may be visually memorable, but it must remain credible in a clinical or technical demonstration.

Avoid the interchangeable AI/SaaS treatment: purple-blue gradients, decorative orbs, excessive rounded cards, glass panels, shadows on every region, giant dashboard hero text, and animation that exists only to attract attention. Avoid a neon cyberpunk treatment that makes the terminology look like a game or security console.

### Brand mark

Reuse `docs/assets/logo/sct-logo.svg` as the single canonical logo source. Preserve its geometry and built-in wordmark, give it an accessible name, and do not place a duplicate text wordmark beside it. The asset uses `currentColor`; when the GUI theme differs from the operating-system preference, apply the same explicit light/dark correction used by `docs/stylesheets/extra.css` so it remains legible. Production must serve or embed the logo locally with the rest of the GUI assets.

### Colour

Define semantic tokens for canvas, surface, raised surface, border, primary text, muted text, focus, selection, success, warning, danger, and graph edges. Assign hierarchy, relationship, status, and mapping colours for meaning rather than decoration. Text, shape, labels, and position must duplicate any distinction made with colour.

Do not give every top-level hierarchy an equally saturated arbitrary colour. Use a restrained family system, with semantic tags and labels carrying the precise distinction. Active/inactive status, IS-A edges, defining attributes, historical associations, and crossmaps require stable and distinguishable treatments in both themes.

### Typography

Use a highly readable humanist sans for clinical language and a precise monospaced face for SCTIDs, ECL, raw values, and compact metadata. Fonts must be bundled with compatible licences if system fonts are insufficient. Establish hierarchy through size, weight, line height, spacing, and colour; do not rely on oversized headings or aggressive letter spacing.

### Structure and elevation

Use borders, spacing, alignment, and changes in canvas tone to organise a dense working surface. Reserve elevation for true overlays such as search suggestions, inspectors, menus, and mobile sheets. Cards are suitable for repeated search results or children when they aid scanning, not as wrappers around every section.

### Motion

Motion explains changes of focus, branch expansion, inspector opening, and graph re-layout. Transitions are short, local, interruptible, and absent under reduced motion. Loading and hover effects must not move surrounding content.

## Information architecture

### Application shell

The first useful desktop composition is a search-and-navigation rail beside a concept workspace. The concept workspace can show the topology and inspector together rather than forcing the user to choose between understanding the graph and reading detail. A slim application bar carries the canonical `sct` logo, edition/release provenance, local/read-only state, theme, and compact global actions.

At phone widths, the interface becomes a single primary surface. Search remains immediately reachable, results replace the workspace until a concept is selected, and secondary detail appears in a full-height view or bottom sheet. No essential action may depend on hover, drag, or a permanently visible sidebar.

### Search

Search behaves like a domain-specific command palette rather than a plain filter box:

- Match preferred terms, synonyms, FSNs, and exact SCTIDs.
- Show the preferred term, semantic tag or hierarchy, SCTID, and match context without making each result noisy.
- Support keyboard movement, selection, dismissal, and restoration of the prior query.
- Preserve plain search as the default and disclose FTS5 or advanced syntax rather than requiring it.
- Provide explicit hierarchy filters and useful no-result guidance.
- Keep result ordering stable and explain any future semantic or frequency-aware ranking.

### Concept identity

The concept header establishes identity at a glance:

- Preferred term as the dominant label.
- Semantic tag and active/inactive status adjacent to the label.
- FSN, hierarchy, and SCTID as supporting identity.
- Parent, child, and descendant counts with honest truncation or approximation language.
- Compact copy actions for SCTID, term, ECL focus expression, and local URL.
- Edition and release provenance available without opening raw metadata.

The remaining workspace progressively exposes synonyms, ancestors/parents, children, defining attributes with role groups, mappings, historical associations, and technical fields.

### Hierarchy navigation

Hierarchy navigation must query hierarchy data rather than infer membership from matching text. It should support top-level hierarchy overview, bounded expansion, counts, multi-parent concepts, and a clear indication when results are truncated. A lineage trail should carry concept IDs so every segment is navigable.

### Knowledge graph

The graph is a focus graph, not an attempt to draw all of SNOMED CT at once.

- The focal concept is visually dominant.
- Direct parents sit consistently above or to the left.
- Children sit consistently below or to the right.
- Defining attributes and role groups occupy a separate axis or lane.
- Additional branches expand on demand with explicit depth and node limits.
- Node positions remain deterministic where possible so repeated visits preserve orientation.
- Labels favour preferred terms, with IDs and full FSNs available on focus or inspection.
- The interface states "50 of 384 children" rather than implying that a truncated set is complete.
- Clicking, keyboard activation, and inspector actions all navigate; dragging is optional enhancement only.
- Fit, zoom in/out, reset, focus next/previous, and return to focal concept have familiar controls and accessible names.

A small measured spike in `GUI-5` should select the layout implementation. The choice may be custom SVG, a vendored graph library, or a small Rust-produced layout, but it must be offline, deterministic enough for explanation, accessible, and performant on bounded graphs.

### Mappings and history

Mappings and history become graph lenses and structured detail rather than raw tables only. The user should be able to see a SNOMED concept's ICD-10, OPCS-4, CTV3, or Read v2 targets, equivalence metadata where present, and history-forwarding path for inactive concepts. These views consume the same typed mapping/history SDK methods as CLI and MCP.

### Compare and query

Later stages add two specialist modes without weakening the search-first explorer:

- **Compare** shows two concepts, nearest shared ancestry, differing parents and defining attributes, and mapping/history differences.
- **Query** provides an ECL workbench with syntax feedback, bounded results, generated links back into the explorer, and optional visual composition for common hierarchy/refinement operations.

These are working tools, not decorative demonstrations. They should ship only with clear user jobs and fixture-backed semantic tests.

## Navigation and state

Concept selection, active lens, graph focus, and useful search state should be represented in the URL. A hash route such as `/#/concept/22298006?view=graph` is sufficient for the localhost SPA and survives refresh without adding server fallback routing. Browser back/forward must replay navigation reliably.

Maintain a visible concept trail for local context, but do not replace browser history with a second incompatible navigation model. State restored from a URL must validate concept IDs and show a recoverable not-found state.

## Technical architecture

### Native adapter

`sct gui` remains a read-only Axum server bound to `127.0.0.1`. It opens the database through the shared read-only path and exposes only bounded query endpoints needed by the interface. It does not become an externally hosted terminology service; `sct serve` owns that job.

### Shared query engine

Move one vertical API slice at a time from direct GUI SQL to typed `sct_rs::sdk::Snomed` methods. The SDK owns terminology semantics and typed records. The GUI adapter owns HTTP status, JSON conversion, route parameters, view-specific aggregation, and presentation limits. Errors use appropriate HTTP status codes and structured bodies rather than returning HTTP 200 with an `error` property.

### Frontend assets

Production assets must be embedded in the binary or shipped with it, with SPDX/REUSE-compatible licensing records. No runtime CDN is permitted. Preserve the no-build frontend while it remains effective. Source may stay in one development HTML file during the initial vertical slice; split embedded HTML, CSS, and JavaScript when doing so improves maintenance and testability rather than as preparatory churn.

If an external graph library or bundled font is proposed, verify its current stable release, source repository, maintenance state, package integrity, runtime weight, and licence before admission. A vendored asset must retain its upstream licence and provenance.

### Relationship to WebAssembly

`R3` is a separate browser-storage and distribution problem. The GUI may later share visual components, interaction contracts, and synthetic browser fixtures with the WASM demo, but `R38` must not wait for WASM or weaken its native SQLite path to imitate browser storage.

## Accessibility and safety

Target WCAG 2.2 AA throughout.

- The complete frequent workflow is keyboard-operable with visible focus and logical order.
- Search suggestions use appropriate combobox/listbox semantics and announce result changes.
- Graph nodes and relationships have a navigable textual equivalent.
- Every drag interaction has button or keyboard alternatives.
- Colour is never the sole indicator of hierarchy, edge type, status, selection, or mapping.
- Touch targets meet WCAG size/spacing requirements and primary mobile controls are normally at least 44 by 44 CSS pixels.
- The interface works at 200% browser zoom without clipping or obscuring controls.
- Reduced motion is respected, and the application remains understandable with motion disabled.
- Loading, empty, no-result, not-found, truncated, unavailable-data, and server-error states have explicit copy and recovery actions.
- Inactive concepts are unmistakable and never presented as current merely because a preferred term exists.
- The UI identifies its edition/release and remains clear that it is a terminology explorer, not diagnostic advice.

## Playwright feedback loop

The implementation loop uses the existing development path:

```bash
cargo run --features gui -- gui \
  --db snomed.db \
  --no-open \
  --dev-html assets/index.html
```

Each substantial frontend slice follows the same loop:

1. Launch against the committed synthetic RF2 fixture for deterministic behaviour and a local licensed database for realistic density.
2. Exercise the frequent path and all states affected by the change.
3. Inspect accessibility snapshots, console output, network requests, and screenshots at 390x844, 768x1024, and 1440x900.
4. Verify keyboard-only navigation, reduced motion, light/dark themes, and 200% zoom where relevant.
5. Implement the smallest coherent improvement, refresh through development mode, and repeat.
6. Turn stable workflow and layout invariants into durable browser checks; avoid brittle pixel-perfect assertions.
7. Keep transient browser artefacts outside the repository or remove them before validation so REUSE remains clean.

The core scenario set is:

| Scenario | Evidence |
|---|---|
| Launch with a valid database | Useful first viewport, provenance visible, zero console errors |
| Search `heart attack` | Relevant preferred/synonym match, keyboard selection, stable loading |
| Open `22298006` | Myocardial infarction identity, hierarchy and relationships agree with the database |
| Open `46635009` | Type 1 diabetes mellitus cross-check agrees with SDK/CLI results |
| Traverse parent, child and attribute | URL/history update, focus remains understandable |
| Open graph and expand a branch | Deterministic bounded layout, truncation disclosed, keyboard alternative works |
| Invalid SCTID and no-result search | Recoverable, concrete empty/error states |
| Phone, tablet and desktop | No overlap, clipping, hidden actions, or horizontal page overflow |
| Network inspection | Only localhost requests; no terminology content or query leaves the machine |

## Completion criteria

The GUI refresh is complete when:

- A new user can search for a clinical phrase, identify the correct concept, understand its place and definition, and traverse a relationship without documentation.
- The search-to-concept vertical slice is visually intentional and recognisably shaped around terminology exploration rather than a generic dashboard.
- The application shell uses the canonical documentation logo, remains legible in both themes, and does not introduce a second brand mark.
- The application makes no external runtime network requests.
- GUI query semantics agree with typed SDK results over the synthetic fixture and known real concepts.
- Browser URLs, refresh, and back/forward reproduce concept navigation.
- Phone, tablet, and desktop layouts pass the documented Playwright scenarios in light and dark themes.
- Keyboard, focus, graph alternatives, reduced motion, contrast, target sizing, and 200% zoom meet the accessibility baseline.
- Graph limits and data truncation are always explicit.
- User documentation matches the final interface and `experimental!` is removed only after the core explorer is stable.

## Build roadmap

Legend: `[x]` done, `[~]` in progress, `[ ]` not started. `R38` is the programme-level item in `spec/roadmap.md`; the `GUI-*` identifiers below are stable delivery stages and must not be renumbered.

- [ ] **GUI-1 - Baseline and browser harness.** Launch the current GUI against the synthetic fixture and a realistic local database; capture the three reference viewports, accessibility tree, keyboard path, console, and network behaviour; record current loading/error/mobile defects; define a repeatable Playwright scenario runner or documented agent loop. Exit when every later stage has reproducible before/after evidence and browser artefacts do not dirty the repository.
- [ ] **GUI-2 - Offline visual foundation.** Remove runtime CDN dependencies; embed the canonical `docs/assets/logo/sct-logo.svg`; establish semantic design tokens, intentional light/dark themes, typography, focus treatment, icons, application chrome, provenance display, and a responsive shell. Exit when the logo and useful first viewport work at phone/tablet/desktop sizes in both themes and network inspection shows localhost-only requests.
- [ ] **GUI-3 - Search, hierarchy, and navigation.** Build the search-first command-palette interaction, correct hierarchy queries, hierarchy filters, keyboard result navigation, hash routes, refresh, browser back/forward, concept trail, and loading/empty/not-found/error states. Exit when the complete search -> concept -> parent/child journey is keyboard-operable, URL-addressable, and fixture-tested.
- [ ] **GUI-4 - Concept workspace.** Redesign concept identity and progressively disclose FSN, semantic tag, status, synonyms, hierarchy, counts, parents, children, defining attributes/role groups, copy actions, and release provenance; migrate the required handlers onto typed SDK records. Exit when Myocardial infarction and Type 1 diabetes mellitus agree with SDK/CLI output and remain readable at all reference viewports and 200% zoom.
- [ ] **GUI-5 - Stable knowledge graph.** Measure and select an offline bounded graph-layout implementation; add deterministic parent/focal/child/attribute topology, explicit expansion and limits, stable transitions, fit/zoom/reset controls, inspector integration, keyboard traversal, textual equivalents, and reduced motion. Exit when graph navigation explains rather than obscures the hierarchy, remains responsive on agreed bounds, and never implies truncated data is complete.
- [ ] **GUI-6 - Mappings and history lenses.** Add typed crossmap and historical-association views for ICD-10, OPCS-4, CTV3, Read v2, and inactive-concept forwarding where the local release contains them. Exit when map/history results match the SDK, unavailable data is distinguished from no match, and inactive concepts clearly lead users to current replacements.
- [ ] **GUI-7 - Compare and ECL workspace.** Add concept comparison with shared ancestry and differing definitions, then a bounded ECL workbench whose results link directly into the explorer. Exit when each specialist workflow has a documented user job, shared semantic fixture coverage, URL-restorable state, and no degradation of the default search-first experience.
- [ ] **GUI-8 - Hardening and graduation.** Complete Playwright workflow coverage, accessibility review, longest-label and high-density testing, performance measurements, security/network inspection, docs and screenshots, packaged-binary smoke tests, and release notes. Exit when all completion criteria above pass and the GUI can honestly lose its `experimental!` label.

Stages are ordered to produce visible value early. `GUI-2` through `GUI-4` form the first polished vertical slice and should ship before graph ambition expands the scope. `GUI-6` and `GUI-7` may be reordered if user evidence favours one, but their identifiers remain stable.

## References

- [`spec/spec.md`](spec.md) - project architecture and offline-first principles.
- [`spec/sdk.md`](sdk.md) - one typed Rust engine and thin GUI adapter direction.
- [`spec/roadmap.md`](roadmap.md) - programme-level `R38` ownership and wider product sequencing.
- [`docs/commands/gui.md`](../docs/commands/gui.md) - current user-facing command documentation.
- [`src/commands/gui.rs`](../src/commands/gui.rs) - current localhost server and API adapter.
- [`assets/index.html`](../assets/index.html) - current embedded interface and development surface.
- [`docs/assets/logo/sct-logo.svg`](../docs/assets/logo/sct-logo.svg) - canonical application and documentation logo.
- [WCAG 2.2](https://www.w3.org/TR/WCAG22/) - accessibility target.
