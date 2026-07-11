# Changelog

All notable changes to `sct` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Releases are grouped from commit messages by [git-cliff](https://git-cliff.org).

## [0.18.4] - 2026-07-10

### Performance

- **serve**: Pool warm read-only connections (~10x concurrent throughput) ([6da60cf](https://github.com/pacharanero/sct/commit/6da60cf72511ab07f03855c9c5aac50a071ff68f))

## [0.18.3] - 2026-07-10

### Bug fixes

- **mcp**: Satisfy clippy::question_mark on a newer stable toolchain ([7bb2429](https://github.com/pacharanero/sct/commit/7bb242948a5d73628da1bf19306ac20eb784b396))

- **mcp**: Avoid full-table schema_version scan on startup ([#32](https://github.com/pacharanero/sct/issues/32)) ([239bf6b](https://github.com/pacharanero/sct/commit/239bf6b3e46eab93909ac30b5c941a7563e85faf))

- **conformance**: Translate check matches any result; no-map fixture ([#30](https://github.com/pacharanero/sct/issues/30)) ([6383a43](https://github.com/pacharanero/sct/commit/6383a43c14d668027c57253917c29373256ec664))

- **map**: Match ICD-10 with dots stripped both sides, not canonicalise ([#31](https://github.com/pacharanero/sct/issues/31)) ([f16dac3](https://github.com/pacharanero/sct/commit/f16dac37979ec3f79706d1486c8f5ae66142b933))

- **cli**: Expand a leading ~/ in every path argument ([f1d5c89](https://github.com/pacharanero/sct/commit/f1d5c89f7437f947793dc20a37259534e54499a9))

- **cli**: Hide the --input alias and animate sqlite's index/FTS phase ([e6c88d9](https://github.com/pacharanero/sct/commit/e6c88d9753a87f723e459a7c9562860f6112008c))

### Build

- Include the interactive TUIs in the default build ([39d8bb9](https://github.com/pacharanero/sct/commit/39d8bb94e60c69185e3ee825e72e494f8b547cc4))

### Documentation

- Add Docker Hub deploy route, refresh benchmarks, fix CLI drift ([faf80c7](https://github.com/pacharanero/sct/commit/faf80c79c274eab4a63e52a5027ca4867a360bcb))

- **roadmap**: AEHRC editor/desktop interop + concept-history feature ([8064ee1](https://github.com/pacharanero/sct/commit/8064ee1a91d98f36856ec7f56433abbf1959926b))

- **roadmap**: Same-hardware sct serve vs Ontoserver benchmark comparator ([a2970c1](https://github.com/pacharanero/sct/commit/a2970c1732dcae640fd31c1016f0c9e60d224d14))

- **roadmap**: Note the on-box loopback run as the definitive Ontoserver bench ([0a63cd3](https://github.com/pacharanero/sct/commit/0a63cd38ea6cd93443f27b835c518b99925529b4))

- **roadmap**: Keep commercial-server comparison private; state public bench policy ([67c7cf6](https://github.com/pacharanero/sct/commit/67c7cf688f0500e251a1de807550518c6a6c08f7))

- **roadmap**: Link open issues; sharpen SAYT into a dual-use design ([a731713](https://github.com/pacharanero/sct/commit/a731713b4bbf2dff106db44f10b35dc8e3f952b0))

- **roadmap**: Stable R## identifiers, soft-wrap, consistent bullets ([55d34e9](https://github.com/pacharanero/sct/commit/55d34e95a021b6ac3ad441ee7454c7be759e2c33))

- **mcp**: Correct --embeddings help - not auto-discovered ([#34](https://github.com/pacharanero/sct/issues/34)) ([01b8df7](https://github.com/pacharanero/sct/commit/01b8df7986ee1288242dc2835665d3e4b44428f4))

- **roadmap**: Refresh open-issues snapshot (tracker now clear) ([0c9925a](https://github.com/pacharanero/sct/commit/0c9925a28b06d66db61d7965a387eddb4adcd1a8))

### Features

- **codelist**: Export --format fhir-json (FHIR R4 ValueSet) ([7c3bcdb](https://github.com/pacharanero/sct/commit/7c3bcdb6f56ef9cf015fed6c1a47ae50f532b76c))

- **bench**: Load.sh - concurrent load-testing harness for sct serve ([284330d](https://github.com/pacharanero/sct/commit/284330da29890e8300aae5ea31fd6cd4f5408d3c))

- **sayt**: Search-as-you-type over the FST index (TUI + stdio + HTTP) ([52bdf9d](https://github.com/pacharanero/sct/commit/52bdf9d134551068c3ce3f7af19b03503e6d35eb))

- **map**: Tolerate undotted ICD-10 input (I219 == I21.9) ([#31](https://github.com/pacharanero/sct/issues/31)) ([61f2948](https://github.com/pacharanero/sct/commit/61f294889d4ecd497dbeb1122d6bc557e8cf421d))

- **serve**: TerminologyCapabilities at /metadata?mode=terminology ([#35](https://github.com/pacharanero/sct/issues/35)) ([6118cd2](https://github.com/pacharanero/sct/commit/6118cd2781f65bfac0652cf3c04848140038a1cf))

- **serve**: FHIR batch Bundle at POST / ([#37](https://github.com/pacharanero/sct/issues/37)) ([6723500](https://github.com/pacharanero/sct/commit/6723500641379adb4dbe44bfa9fbed2390942a54))

- **cli**: Rename --input to --ndjson on NDJSON-consuming commands (R2) ([616f5b8](https://github.com/pacharanero/sct/commit/616f5b8ceeaf0944059a104c01419ee071368fab))

- **cli**: Live progress bars with ETA for the long-running builds (R1) ([6449697](https://github.com/pacharanero/sct/commit/6449697a762020bb57ac9f0d326e30e7e3121573))

- **ndjson**: Per-section progress bars across the RF2 load and build ([9a92da1](https://github.com/pacharanero/sct/commit/9a92da133426443db79d0cfffe62f2a030695d9c))

## [0.18.2] - 2026-07-09

### CI

- **release**: Publish a multi-arch sct image to Docker Hub on release ([d693c66](https://github.com/pacharanero/sct/commit/d693c6645a018a9edddeb810b0ebb6ab32c2305b))

### Chores

- Fix dangling doc sentence and zensical emoji extension path ([f7a839f](https://github.com/pacharanero/sct/commit/f7a839f028e801e5dbdac62d36e75855e94d05a4))

### Documentation

- **spec**: Commit to Compose + separate Caddy (Option A) in deployment plan ([9687bd8](https://github.com/pacharanero/sct/commit/9687bd860176e6eb511c67fb2e3e625f1828ffd5))

- **semantic**: Replace fabricated examples with verified output; set honest expectations ([15eb247](https://github.com/pacharanero/sct/commit/15eb247d3190344cee95598a0049fea117cfd39f))

- Note 4096-char description-length readiness in trud/ndjson ([1baae7c](https://github.com/pacharanero/sct/commit/1baae7cb119e4c1b1ab3f24afae8ede79317881f))

- **roadmap**: Remove completed items, verify claims, tidy stray note ([a71fe84](https://github.com/pacharanero/sct/commit/a71fe84a1d6935a3b9c676b39dd56415ca306850))

- **deploy**: Rewrite the self-host guide for the Caddy-fronted stack ([d921bf1](https://github.com/pacharanero/sct/commit/d921bf1d92cf7b3fa29c4dabb0eb62580c8dd878))

### Features

- **embed**: Stamp model + text scheme into Arrow metadata; verify at query time ([3654fb9](https://github.com/pacharanero/sct/commit/3654fb9a435161e7b8d674802567036591a23ef7))

- **deploy**: TLS reverse proxy for sct serve via Caddy + Compose ([0820924](https://github.com/pacharanero/sct/commit/0820924ace553e99f5f38c5f59e4d5ccdab708e9))

## [0.18.1] - 2026-07-08

### Chores

- Rename specs/ to spec/ for consistency across repos ([8169ddc](https://github.com/pacharanero/sct/commit/8169ddc35e0532a2a6843fa49049c852c347720f))

### Documentation

- **roadmap**: Add concurrent load-testing harness item for sct serve ([c603896](https://github.com/pacharanero/sct/commit/c603896fea6322d1218cd49c3461db919a0dbf67))

- **roadmap**: Note DB-wide INTEGER columns + optional one-pass build as potential ([9ce66ad](https://github.com/pacharanero/sct/commit/9ce66adfbc807b712b3da3a5dfe1ccb23179097c))

- **spec**: Add sct serve appliance deployment design ([d517943](https://github.com/pacharanero/sct/commit/d51794366d1aa679a141e95ee34c081db6052381))

### Performance

- **tct**: U64 closure + INTEGER concept_ancestors columns ([6babaa8](https://github.com/pacharanero/sct/commit/6babaa8975eb938e98971c829b913472e667e8e1))

## [0.18.0] - 2026-07-07

### CI / dependencies

- **deps**: Upgrade axum 0.7->0.8, rusqlite 0.39->0.40, sha2 0.10->0.11, toml 0.8->1.1 ([e15cd60](https://github.com/pacharanero/sct/commit/e15cd60fdcb817d0017699d26e9eda1814d83e7b))

### Performance

- **ecl**: Index-probe small refinements, bulk-build sets, buffer stdout ([3c47992](https://github.com/pacharanero/sct/commit/3c4799208dd787b1d4820978bf3842b93891ae56))

## [0.17.1] - 2026-07-07

### Bug fixes

- **build**: Make optional deps available on Windows; add Windows CI check ([a8ac793](https://github.com/pacharanero/sct/commit/a8ac7934a1bba4b6f0620aeba6a708939d948300))

## [0.17.0] - 2026-07-06

### Build

- Include sct serve in the default build ([4fd6945](https://github.com/pacharanero/sct/commit/4fd69459c99e98b9715787101618e486fea8e6a1))

### CI

- Gate FHIR conformance on every push (sct serve vs committed fixture) ([c00b390](https://github.com/pacharanero/sct/commit/c00b3901d802de7c058cec9ffce14b7daea40bfb))

### CI / dependencies

- **deps**: Bump actions/cache from 6.0.0 to 6.1.0 ([73fe86b](https://github.com/pacharanero/sct/commit/73fe86b5d855c68f11f789cdce2604fe7a8ef926))

### Chores

- **dist**: Point Scoop at shared pacharanero/scoop bucket ([eefff15](https://github.com/pacharanero/sct/commit/eefff152e328ebb7c03ce3d5dc25ee1a83cf0175))

- **s/install**: Add --serve and --dmwb feature flags, plus --help ([72121b6](https://github.com/pacharanero/sct/commit/72121b64780ab05a92c615bb78a5cb3942d8329c))

- **s/install**: Make --full mean everything and print the feature set ([ad1d82d](https://github.com/pacharanero/sct/commit/ad1d82dfe2e88855b0f2cfeb213907c880d060d6))

- Ignore all benchmark reports, not just .md ([4ec5890](https://github.com/pacharanero/sct/commit/4ec5890425adb3bb0015dbeaef37ae5df063474f))

### Documentation

- **roadmap**: De-duplicate TODO vs Distribution; update transcode/crosswalk refs to sct map ([ea820e0](https://github.com/pacharanero/sct/commit/ea820e027cedcd61f3ee4c5f754f356334c9dbf3))

- Replace ASCII data-flow diagrams with Mermaid; enable Mermaid rendering ([e164dd0](https://github.com/pacharanero/sct/commit/e164dd0db90d77013861c31bb37226377671832b))

- **roadmap**: Add externally-verified FHIR conformance (HL7 FHIR Validator) ([9569a15](https://github.com/pacharanero/sct/commit/9569a15310df3b8fb77acb6b7a35b74f886ec138))

### Other

- Merge dependabot #29: bump actions/cache 6.0.0 -> 6.1.0 ([6e87dd8](https://github.com/pacharanero/sct/commit/6e87dd8d2f24209e801f57fcdf58e5d7c840c77f))

- Add s/snowstorm-lite one-command comparator setup ([90dc954](https://github.com/pacharanero/sct/commit/90dc95488f75710ef90c6ebffdafb3492330e563))

- **conformance**: FHIR-correct fixes surfaced by testing against Ontoserver ([fb6edde](https://github.com/pacharanero/sct/commit/fb6eddee1395aef0c245965e98e24d5646df556d))

- Add readable bar-chart output; report leads with chart + code-fenced table ([e671090](https://github.com/pacharanero/sct/commit/e671090f4019dd24296695a1ea7e668517c176d1))

- Fix stale stddev (subshell bug) + de-emphasise summed total ([14f59c0](https://github.com/pacharanero/sct/commit/14f59c083b24f2ebcd3549bed22a30135269c981))

- Add --sct-fhir mode for like-for-like sct-serve-vs-FHIR comparison ([4ad2ae1](https://github.com/pacharanero/sct/commit/4ad2ae1ae8c723cb8807beabca97214b2d21fe80))

- Fix set -u crash on --write-benchmarks in --sct-fhir mode ([ba751e0](https://github.com/pacharanero/sct/commit/ba751e0b85a2541e8514a1fc20b07ba29a5d1066))

- Fill in ancestor depth in FHIR-vs-FHIR mode ([17a26ab](https://github.com/pacharanero/sct/commit/17a26ab9b3ec5fbf09eb8b5786923a9bf66e4c6e))

- Add in-process profiling harness for core query paths ([de88b52](https://github.com/pacharanero/sct/commit/de88b522c226335f1345204f0ee1265e1a843b41))

### Performance

- **ecl**: Switch IdSet to BTreeSet<u64> - 2.3x faster large expansions ([ce417d4](https://github.com/pacharanero/sct/commit/ce417d485bb02c71fc8405f982423acda128d51f))

### Refactor

- **benchmarks**: Consolidate bench/ + benches/ + benchmarks/ into one dir ([2b1b7a5](https://github.com/pacharanero/sct/commit/2b1b7a5a8af3dad928f2d0af3b87547c1f86b4d4))

## [0.16.0] - 2026-07-05

### Chores

- Release 0.16.0 ([3a467e4](https://github.com/pacharanero/sct/commit/3a467e410370a46c72f639d3830d07a8fd8720be))

### Features

- **cli**: Standardise output on --format text|json|yaml; --template for line templates **[breaking]** ([9f3f714](https://github.com/pacharanero/sct/commit/9f3f714021d73a14f4ff444c986f6c3b1f1885fa))

## [0.15.0] - 2026-07-05

### Chores

- Release 0.15.0 ([f743dc9](https://github.com/pacharanero/sct/commit/f743dc9e8fb968c0f413ed973652abc17914a9dd))

### Features

- **cli**: Unify cross-terminology mapping under 'sct map' ([464422e](https://github.com/pacharanero/sct/commit/464422e9e57c37fa100d7bf106a2c8f788512738))

## [0.14.0] - 2026-07-05

### Bug fixes

- Fix CI clippy and reuse checks ([b8bc590](https://github.com/pacharanero/sct/commit/b8bc5903cecc39ede722754fe750f9b0a639d232))

### CI / dependencies

- **deps**: Bump softprops/action-gh-release from 3.0.0 to 3.0.1 ([#27](https://github.com/pacharanero/sct/issues/27)) ([6b5b87d](https://github.com/pacharanero/sct/commit/6b5b87d35e7ac1654defa61a2c6af289d33eca04))

- **deps**: Bump actions/cache from 5.0.5 to 6.0.0 ([#26](https://github.com/pacharanero/sct/issues/26)) ([406bb56](https://github.com/pacharanero/sct/commit/406bb5607568d2d62cc98c736c172d54b09bc12f))

- **deps**: Bump actions/setup-python from 6.2.0 to 6.3.0 ([#25](https://github.com/pacharanero/sct/issues/25)) ([4452b43](https://github.com/pacharanero/sct/commit/4452b433521984811efd3911ce73128fed4b8740))

- **deps**: Bump actions/checkout from 6.0.2 to 7.0.0 ([#24](https://github.com/pacharanero/sct/issues/24)) ([fb3718b](https://github.com/pacharanero/sct/commit/fb3718b92856c272e0b2d57b850eb00d8f374c57))

### Chores

- Release 0.14.0 ([aa56e7d](https://github.com/pacharanero/sct/commit/aa56e7d9932862bf6316d192519d952f99eb22c3))

### Documentation

- Add docker terminology server quickstart ([58f6c63](https://github.com/pacharanero/sct/commit/58f6c639ea4634d7a7f89dd71886bdfe7a4e1027))

- Drop Runme/walkthrough scripts; roadmap: add AI-benchmark + prior-art ideas ([d15fcc0](https://github.com/pacharanero/sct/commit/d15fcc0511779f1df4446bde44ffa3619255c149))

### Features

- **cli**: Add completions installer ([bbea671](https://github.com/pacharanero/sct/commit/bbea671f19ef884e654ba79c4a97a56439b393f7))

- **cli**: Add sct diagram and sct ecl compress (slice 1) ([a7d34c0](https://github.com/pacharanero/sct/commit/a7d34c0af2a18c04904b430c29e766a53f7192fa))

### Other

- Add docker compose terminology server ([439ab63](https://github.com/pacharanero/sct/commit/439ab6318238e186b1301bcb61800b849a8ba38c))

- Improve docs server port selection ([9cd505f](https://github.com/pacharanero/sct/commit/9cd505f8a7045f2d870c17478139e2af2cbb67fe))

- Add FHIR conformance benchmark harness ([e8c8244](https://github.com/pacharanero/sct/commit/e8c82446c77e88fd7e0ee2ccefb184e183d9ede8))

- Clarify DMWB Read v2 import status ([8082dee](https://github.com/pacharanero/sct/commit/8082dee2f6c7a7779da7da861e9fbf9cb53853fb))

- Import Read v2 item 9 maps ([ac3b0a5](https://github.com/pacharanero/sct/commit/ac3b0a5d093895471bea8cb1c9c55afc05608302))

- Bump version to 0.13.0 ([75f419b](https://github.com/pacharanero/sct/commit/75f419bca45b80cc0ec078065329aa8bef7db569))

- Switch Homebrew releases to shared tap ([8ddfa5a](https://github.com/pacharanero/sct/commit/8ddfa5aa394d6678d1c7b3b2bc37be73f5dac026))

- Update roadmap after multi-terminology work ([540ad57](https://github.com/pacharanero/sct/commit/540ad574d52a45d9602fdbb61b5d8213579563de))

- Allow minor and major version bumps ([b005255](https://github.com/pacharanero/sct/commit/b005255a761ef5b1e51cd560db8f3029a26ba808))

- Add ICD support roadmap notes ([03305ed](https://github.com/pacharanero/sct/commit/03305ed6599ba76fbe6c9ca98f8b66567b33eba8))

- Merge pull request #28 from pacharanero/codex/completions-install ([4d563d8](https://github.com/pacharanero/sct/commit/4d563d84ad4ba4a26a41586455f8998464d85258))

- Bump arrow and parquet to remove thrift ([f61c5e7](https://github.com/pacharanero/sct/commit/f61c5e7239238623177bd3cb9716309033b98061))

- Document trud subscription summary ([9830c24](https://github.com/pacharanero/sct/commit/9830c2441b35c118b89533f834dc23c2b8f5157e))

## [0.12.0] - 2026-06-24

### Documentation

- Add DMWB walkthrough ([3f0f492](https://github.com/pacharanero/sct/commit/3f0f492e8ee2e3e790b9a3c531c12d038158b5ad))

- Update roadmap status ([8cbd11a](https://github.com/pacharanero/sct/commit/8cbd11a559aba5c01a204066615e621f3fb81514))

- Document Read v2 item 9 source ([a448180](https://github.com/pacharanero/sct/commit/a448180cf0fa2cacec6847f583b922a40b6173e7))

### Other

- Generalise crossmap storage ([9a7946a](https://github.com/pacharanero/sct/commit/9a7946a0289947c59b2136e6c25199048ae93961))

- Tidy specs layout ([56af08d](https://github.com/pacharanero/sct/commit/56af08dc5afb0b7022775b0d784a4cedfdc0d3b9))

- Bump version to 0.12.0 ([e8b60d7](https://github.com/pacharanero/sct/commit/e8b60d792074e5cb60c9a03af063036f1c44377c))

## [0.11.0] - 2026-06-22

### Chores

- Release 0.11.0 - trud --pipeline honours ndjson shaping flags ([48c24a3](https://github.com/pacharanero/sct/commit/48c24a34390a8161ffbb0d280e6ee6ca0a27948b))

### Features

- **trud**: Pipeline --include-inactive, --refsets and --locale into ndjson ([e4d4b89](https://github.com/pacharanero/sct/commit/e4d4b899745a01731e379e6d83303a857223660c))

## [0.10.1] - 2026-06-22

### Bug fixes

- **ndjson**: Make --include-inactive actually include inactive concepts ([4786599](https://github.com/pacharanero/sct/commit/47865996ed8996399ed966045f57dfc228e8b17b))

### Chores

- Release 0.10.1 - fix --include-inactive no-op ([49f3a68](https://github.com/pacharanero/sct/commit/49f3a68eff480ea3f6c671dc6c8899c743e042bd))

## [0.10.0] - 2026-06-12

### Chores

- Release 0.10.0 - cross-terminology mapping (DMWB-replacement core) ([ac4df8f](https://github.com/pacharanero/sct/commit/ac4df8fab188274616c72ce54911361621968116))

### Documentation

- **spec**: Cross-terminology mapping + DMWB replacement design ([d40e711](https://github.com/pacharanero/sct/commit/d40e7118a05e9a3de921ad3b0fda36b3646a7b54))

### Features

- **maps**: RF2-native SNOMED CT -> ICD-10 / OPCS-4 crossmaps (Phase 1a) ([b45cf0d](https://github.com/pacharanero/sct/commit/b45cf0dd65f13e58868ef02b709dd9713c9e8398))

- **maps**: RF2-native concept history / inactive forwarding (Phase 1b) ([625b335](https://github.com/pacharanero/sct/commit/625b3355806bc30a6294af898c8dd322c2a85ba6))

- **transcode**: Sct transcode - cross-terminology code mapping (Phase 2) ([f89fd81](https://github.com/pacharanero/sct/commit/f89fd81dcf1f1aa6cfdc7583a110b3468bd598cf))

- **crosswalk**: Sct crosswalk - all equivalents of a code (Phase 2 complete) ([b080409](https://github.com/pacharanero/sct/commit/b080409b7a5cc046ca44877ac93d9b2afe5c5d7b))

- **serve**: ConceptMap/$translate over the crossmaps (Phase 4) ([5737b31](https://github.com/pacharanero/sct/commit/5737b31a9ad488f100389b6be6a01987edcd4f8e))

- **codelist**: Cross-terminology export incl. ICD-10/OPCS-4 (Phase 5) ([e98135a](https://github.com/pacharanero/sct/commit/e98135af5c7520acddff1eb91898b485c390b9b6))

- **dmwb**: DMWB .mdb introspection via jetdb; Read v2 import gate fails (Phase 3) ([2d0f060](https://github.com/pacharanero/sct/commit/2d0f060caf4e77668086b1d6fc1a1921a2632a12))

## [0.9.3] - 2026-06-08

### Chores

- Sync Cargo.lock to 0.9.2 ([e988f77](https://github.com/pacharanero/sct/commit/e988f77f6ccf416dd7e908a0892a8e7683840bf0))

- REUSE-compliant SPDX headers across the repo ([4b30f2e](https://github.com/pacharanero/sct/commit/4b30f2ee34c0b86bf1518164331864c2d61e29fb))

- Release 0.9.3 (AGPL crate publish + SPDX headers) ([be877e8](https://github.com/pacharanero/sct/commit/be877e8eaa09949379ed7db53bcddf107ebd23aa))

## [0.9.2] - 2026-06-08

### Chores

- License under AGPL-3.0-or-later (match LICENSE and README) ([cd363d2](https://github.com/pacharanero/sct/commit/cd363d2f36a97a0b48f183cfa562e83fe0dd6254))

## [0.9.1] - 2026-06-08

### Build

- **release**: Produce .deb, .rpm, .dmg, and bare .exe artifacts ([7584ef4](https://github.com/pacharanero/sct/commit/7584ef49db59b2dfabbdf639d40e7633e8187c04))

### Documentation

- Rich content-tabs install section; enable pymdownx.tabbed ([097169b](https://github.com/pacharanero/sct/commit/097169b826122b7c426ba1bc8aab988d2e41e8d7))

## [0.9.0] - 2026-06-07

### Features

- **serve**: Serve .codelist files as stored FHIR ValueSets ([2bb2b8a](https://github.com/pacharanero/sct/commit/2bb2b8a897d5783d7f21c5be8c19ea7509950b14))

## [0.8.0] - 2026-06-07

### Chores

- Drop tracked .downloads/ placeholder; fix stale doc path ([062581d](https://github.com/pacharanero/sct/commit/062581de88f774aa2b7df337d59ea535cfd4863e))

### Features

- **codelist**: Composable codelists via `includes:` ([e8a46c7](https://github.com/pacharanero/sct/commit/e8a46c760a58b916cef4328071de29dff2593485))

## [0.7.2] - 2026-06-04

### Performance

- **serve**: SQL fast path for single-operator $expand ([887d99d](https://github.com/pacharanero/sct/commit/887d99d5b7af0ad98ef4adc34a2de7728c06bf01))

## [0.7.1] - 2026-06-04

### Performance

- **ecl**: Use the transitive-closure table for <</>>; warn when absent ([bd9628f](https://github.com/pacharanero/sct/commit/bd9628fef46ca8c4f2b592dd82e7622313d4c0ad))

## [0.7.0] - 2026-06-04

### Documentation

- Prune completed items from roadmap; document schema v4 in ndjson docs ([fa4a06c](https://github.com/pacharanero/sct/commit/fa4a06c988de40468fa7a076168631323a346f73))

### Features

- **serve**: FHIR R4 terminology server (Phase 1) ([7b157c1](https://github.com/pacharanero/sct/commit/7b157c16cdc99d6e48e4de5f68d285e9bfdad85f))

### Tests

- Add synthetic licence-free RF2 fixture + full-pipeline end-to-end tests ([d53724b](https://github.com/pacharanero/sct/commit/d53724b7448dbd0d9bcca5b6377a8d33aab990c8))

## [0.6.2] - 2026-06-04

### Documentation

- Add "Why code lists?" page; split "Why Build This?" nav section ([2ad064d](https://github.com/pacharanero/sct/commit/2ad064d112c527138015f865cdbdc7c1042cbfaf))

- Cite the QRISK2 code-selection incident as numbered references ([c206dec](https://github.com/pacharanero/sct/commit/c206dec01ce1ded181aa3e3ba8e1b6eb8f7f941f))

- Move the "why" pages into a docs/why/ folder ([fdaa12b](https://github.com/pacharanero/sct/commit/fdaa12b14d15881ebae5185a912d4aa2db69735e))

- **nav**: Group commands into task-based top-level sections ([f339472](https://github.com/pacharanero/sct/commit/f33947219a1eac15dadff1d1e3df55577fff28b4))

### Other

- Removed every last bastard emdash ([7cb9a88](https://github.com/pacharanero/sct/commit/7cb9a886733a6c24563a2068c18fa1cb2fcc39c6))

### Refactor

- **codelist**: Drop the unimplemented `publish` command (Q7) ([3ff5ce7](https://github.com/pacharanero/sct/commit/3ff5ce7c4c1f0fe3f6117785a0449f0119052393))

## [0.6.1] - 2026-06-04

### Features

- Add --ids (pipe-friendly) mode to read-side commands ([01b32a5](https://github.com/pacharanero/sct/commit/01b32a595703d4a7e12ecad0bc1ead62c3a32c8a))

## [0.6.0] - 2026-06-04

### Features

- **ecl**: Add composable `sct ecl expand` + stdin for `codelist add` ([712663b](https://github.com/pacharanero/sct/commit/712663bca691818d5e56031ecc41872cb9ed977b))

## [0.5.2] - 2026-06-04

### Bug fixes

- **ndjson**: Make --locale dialect-aware via language reference set id ([37f118f](https://github.com/pacharanero/sct/commit/37f118f0e1ef8cc4add46105260e1c6127007173))

## [0.5.1] - 2026-06-04

### Documentation

- **roadmap**: Add composable codelists idea ([628d5eb](https://github.com/pacharanero/sct/commit/628d5eb35b346a3f83c7b2f6a507be8c7f41e367))

- **roadmap**: Add interactive search-as-you-type mode ([67a9d93](https://github.com/pacharanero/sct/commit/67a9d9341ebd003fffbd056bb16e86bd95c14faf))

- Project review tidy-up (roadmap, docs, specs, coverage) ([230e3af](https://github.com/pacharanero/sct/commit/230e3af9cfc1cc308c358ebf0a3c0dc763ad21d0))

## [0.5.0] - 2026-06-03

### Features

- **ecl**: ECL parser + evaluator, wired into `codelist add --ecl` ([76c89f0](https://github.com/pacharanero/sct/commit/76c89f0f350e47a0ee6b3b5ad5d55e4d8f2eca06))

## [0.4.1] - 2026-06-03

### Performance

- **fst**: Delta-varint posting lists + optional display side-tables ([f096571](https://github.com/pacharanero/sct/commit/f096571623c4ce1915cd0e127df060aff3e4c271))

## [0.4.0] - 2026-06-03

### CI

- Auto-tag on Cargo.toml version bump ([732390e](https://github.com/pacharanero/sct/commit/732390ea814e810d7a9bd7fcb09f4da38c6af825))

- Pin GitHub Actions to SHA hashes with version comments ([ff275a1](https://github.com/pacharanero/sct/commit/ff275a1ce9b1fd0887790fbc0679265893212619))

### CI / dependencies

- **deps**: Bump rustls-webpki in the cargo group across 1 directory ([#17](https://github.com/pacharanero/sct/issues/17)) ([8403eb9](https://github.com/pacharanero/sct/commit/8403eb9ef9522acb71e58148db7c40a28cdca18d))

- **deps**: Bump rand from 0.8.5 to 0.8.6 ([6563321](https://github.com/pacharanero/sct/commit/6563321f9d4f9880d3945087dede9977fad7a056))

### Documentation

- Open zensical server in browser by default ([6a7ff0e](https://github.com/pacharanero/sct/commit/6a7ff0ecbc8c167e012b6b3d22511dacfd2d2311))

- Add documentation for SNOMED CT's GPS and IPS releases ([936bcb3](https://github.com/pacharanero/sct/commit/936bcb3cdae268de42aa7fe8456422df938bfffb))

- Add repository URL and name to mkdocs configuration ([02a7415](https://github.com/pacharanero/sct/commit/02a7415a1ca3b847c76d1cc465be4009e51ee025))

### Features

- Add specification for `sct static` command to generate static FHIR terminology service ([06a1a69](https://github.com/pacharanero/sct/commit/06a1a691db6dfd8992b6d8537ac1720b16feff7d))

- Add experimental FST-backed lexical index (sct fst) ([b2bcbbc](https://github.com/pacharanero/sct/commit/b2bcbbc7a16cfde6f49b880789f171bc977725ea))

### Other

- Tweaks to trud.md - closes #18 ([927bccb](https://github.com/pacharanero/sct/commit/927bccbd3781dc926b1be52d084b19002a38daed))

### Refactor

- Streamline test functions and introduce env-mutation lock for isolation ([350b631](https://github.com/pacharanero/sct/commit/350b6317a33a4e71bde8981bf1953afcffd46de2))

## [0.3.11] - 2026-05-20

### CI / dependencies

- **deps**: Bump actions/upload-pages-artifact from 4 to 5 ([#13](https://github.com/pacharanero/sct/issues/13)) ([6ba2699](https://github.com/pacharanero/sct/commit/6ba269921884e999aa469b9c84acffa14481661d))

- **deps**: Bump actions/upload-artifact from 7.0.0 to 7.0.1 ([#14](https://github.com/pacharanero/sct/issues/14)) ([0217354](https://github.com/pacharanero/sct/commit/0217354a5d6d8e26d41baec717380852434cdf5f))

- **deps**: Bump softprops/action-gh-release from 2.6.1 to 3.0.0 ([#15](https://github.com/pacharanero/sct/issues/15)) ([ab0b945](https://github.com/pacharanero/sct/commit/ab0b945beb6aa5d7d1109f7155fe9ce84936a18a))

- **deps**: Bump actions/cache from 5.0.4 to 5.0.5 ([#16](https://github.com/pacharanero/sct/issues/16)) ([b4f496d](https://github.com/pacharanero/sct/commit/b4f496db9b44ca3e7074b6d0ebb6e2388d8bdaef))

### Documentation

- Link walkthrough installation to README instead of duplicating ([d73063b](https://github.com/pacharanero/sct/commit/d73063b6b8f8ef6742a16f5620b7c3372f56e8c3))

### Other

- Replace unmaintained serde_yml with serde_yaml_ng; bump ratatui to 0.30 ([b3252a5](https://github.com/pacharanero/sct/commit/b3252a5ee2b5466192822191dfd094d7b659cf04))

- Capture source release in NDJSON header and SQLite metadata table ([888d639](https://github.com/pacharanero/sct/commit/888d63944a3862f1e4cb51415e5aa510e23bc586))

- Surface in lookup, lexical, and refset with --provenance flag ([f8d17d9](https://github.com/pacharanero/sct/commit/f8d17d9f8cd30572b8dc6b2050a926c770663a1d))

- Extend to embed/semantic, mcp, and codelist auto-population ([ff43cfd](https://github.com/pacharanero/sct/commit/ff43cfd7222e04899888033586586ca7323cf4c8))

- --include-maps adds crosswalk columns on export ([e105ca3](https://github.com/pacharanero/sct/commit/e105ca3fd7ca038dc298a6a69ebe48558352020b))

- Sketch exploration/data-science surfaces ([985b2b4](https://github.com/pacharanero/sct/commit/985b2b4b2138022d307bf2df24a58afe9921e3d8))

- Add first MMR vaccination procedures codelist ([0ffc98e](https://github.com/pacharanero/sct/commit/0ffc98e2ee18f634928104ccae3ccc15fac287d3))

- Unify db/embeddings/config discovery across all commands ([96f5dce](https://github.com/pacharanero/sct/commit/96f5dce7ae7b535a09b9cb3ee8b7b56acc659c56))

## [0.3.10] - 2026-04-11

### Bug fixes

- Exit cleanly on SIGPIPE instead of panicking ([2d47d05](https://github.com/pacharanero/sct/commit/2d47d050c9ec999c3c5207a0bc5ad03c761d0dd7))

- Clear clippy --all-targets warnings in trud.rs ([5922eba](https://github.com/pacharanero/sct/commit/5922ebaa14aaa8231c13537c9e74d686656b3d80))

### Chores

- Add pre-commit hook running fmt + clippy ([d65b417](https://github.com/pacharanero/sct/commit/d65b417d29fa17018b36113cdac5f67d35f3fa34))

### Documentation

- **roadmap**: Document --refsets all as a future job ([8b8768b](https://github.com/pacharanero/sct/commit/8b8768bb26af1f30fb7b1e8b511ac06bc20e536f))

- Split walkthrough into focused pages and add refset section ([f17e1b4](https://github.com/pacharanero/sct/commit/f17e1b4d20e230f2fd6ebc14ffb00e9a60d0ce6b))

- Remove old monolithic walkthrough.md ([d03d362](https://github.com/pacharanero/sct/commit/d03d362c99b55c907a95a7335fd8b067af32970e))

- Update README ([3d99cda](https://github.com/pacharanero/sct/commit/3d99cda556518bc16f7f98904995d0bab8c12ab4))

### Features

- Add Simple refset support end-to-end ([76e6f0f](https://github.com/pacharanero/sct/commit/76e6f0ffa3b1c407976bf2c348842daa0c966b3b))

- Configurable one-line concept listing format ([95fe2d2](https://github.com/pacharanero/sct/commit/95fe2d28c9151fa1fc4bc55ce11b49347d3a98d1))

- Update installation instructions and add shell one-liners for easier setup ([894bbaa](https://github.com/pacharanero/sct/commit/894bbaa4841eaf180c55a6f96b4a05745064840a))

### Other

- Add install.ps1 and auto-bump Homebrew tap + Scoop bucket on release

Tier 2 distribution wiring. Each release now fans out to package managers
automatically, on top of the Tier 1 GitHub Releases + crates.io + curl|sh
flow already in place.

install.ps1: Windows PowerShell installer, the counterpart to install.sh.
Auto-detects architecture, looks up the latest release tag via the GitHub
API, downloads sct-windows-x86_64.zip plus SHA256SUMS, verifies the
checksum, extracts to $env:LOCALAPPDATA\sct\bin by default (overridable
via $env:SCT_INSTALL_DIR), and offers to add the install directory to
the user PATH.

  # One-liner install on Windows:
  iwr -useb https://raw.githubusercontent.com/pacharanero/sct/main/install.ps1 | iex

release.yml: new `update-taps` job that runs after the GitHub release is
published. It pulls the just-published SHA256SUMS, extracts each of the
five per-target hashes, and rewrites:

  - Formula/sct.rb          in pacharanero/homebrew-sct
  - bucket/sct.json         in pacharanero/scoop-sct

then commits and pushes. Both target repos are cloned with a PAT stored
as the TAP_REPOS_TOKEN secret — this must be a token with write access
to both repos. Without the secret, the job will fail visibly but the
main release (GitHub Release + crates.io) will still succeed, since
update-taps has `needs: release` and is not itself a prerequisite for
anything downstream.

The Scoop manifest also carries its own `checkver` / `autoupdate` block,
so Scoop's own update machinery can pick up new versions without this
workflow — the workflow just makes the update immediate rather than
eventually-consistent.

Roadmap: marked the distribution checklist items complete and added a
"Future distribution work" sub-list for the harder items (macOS
notarization, Windows Authenticode signing, .deb/.rpm, winget, nixpkgs)
that still need either money or patience.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com> ([9ad8771](https://github.com/pacharanero/sct/commit/9ad8771a5bf0915b3fae2ab8e0e3b48c5bedac07))

- Removed old, completed work from spec.md ([6589b6b](https://github.com/pacharanero/sct/commit/6589b6b33b24462bad29865ec57b051ad3f3fbf5))

- Clarify cargo-binstall is a separate plugin ([de2eec8](https://github.com/pacharanero/sct/commit/de2eec8bb31b325907a746fe9fc0686b2864dc62))

- Bump version to 0.3.10 ([ccc12a1](https://github.com/pacharanero/sct/commit/ccc12a1d59b80e422059ecbcfec82dc8dde533b5))

### Refactor

- Unify concept-list output via format module + shared open_db ([922ac4b](https://github.com/pacharanero/sct/commit/922ac4b455d23cff7eb59d5efe94f6141f66eb56))

## [0.3.9] - 2026-04-10

### Bug fixes

- Update path to Cargo.toml and adjust git add command ([c6f2cc0](https://github.com/pacharanero/sct/commit/c6f2cc0a417816d3409f2f0d343d09e545a01748))

- Update readme path in Cargo.toml ([01efae6](https://github.com/pacharanero/sct/commit/01efae6c34200fc181a004409c90ff68a15455a9))

### Other

- Add Windows + Linux ARM builds, SHA-256 checksums, and curl|sh installer

Extends the release workflow's matrix to build five targets on tag push:
  - x86_64-unknown-linux-musl   → sct-linux-x86_64.tar.gz
  - aarch64-unknown-linux-musl  → sct-linux-aarch64.tar.gz   (new, native
                                   ubuntu-24.04-arm runner, no cross-compile)
  - aarch64-apple-darwin        → sct-macos-aarch64.tar.gz
  - x86_64-apple-darwin         → sct-macos-x86_64.tar.gz
  - x86_64-pc-windows-msvc      → sct-windows-x86_64.zip     (new, zipped
                                   via PowerShell Compress-Archive)

The release job now generates a single SHA256SUMS file covering every
artefact and attaches it to the GitHub release alongside the tarballs.

install.sh is a POSIX-sh installer that users can pipe from curl:

  curl -fsSL https://raw.githubusercontent.com/pacharanero/sct/main/install.sh | sh

It auto-detects OS and architecture, fetches the latest release tag, downloads
the matching tarball plus SHA256SUMS, verifies the checksum before install,
and drops the binary at $HOME/.local/bin (override via $SCT_INSTALL_DIR).
Falls back between curl/wget and sha256sum/shasum.

Cargo.toml grows a [package.metadata.binstall] block so `cargo binstall
sct-rs` can download the prebuilt artefacts instead of compiling from source.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com> ([b79ba76](https://github.com/pacharanero/sct/commit/b79ba76ffc4a4178a602d9ed434c2b3832a1b97c))

- Bump version to 0.3.9 ([cb73f57](https://github.com/pacharanero/sct/commit/cb73f57765d0f85360ba450cd0b0c55a99fbcbf1))

### Refactor

- Integration tests for RF2 parsing and record building ([da03bfc](https://github.com/pacharanero/sct/commit/da03bfc067c4126598fca1f56b09b74468038f01))

### Styling

- Cargo fmt ([40d3d9e](https://github.com/pacharanero/sct/commit/40d3d9e4d890d7e86a53e9131b6b20e9a79853f2))

## [0.3.8] - 2026-04-10

### Bug fixes

- Fix install path in s/install ([519cf38](https://github.com/pacharanero/sct/commit/519cf38d90af598ef3e3c776de0b73ee3e6ed31e))

- Fix bug in SQL code used for demo ([4c8db51](https://github.com/pacharanero/sct/commit/4c8db51dc0d1bb8747a0e0d299296ecd83073d4f))

- Fixes to benchmarking scripts ([e3a78f1](https://github.com/pacharanero/sct/commit/e3a78f13ce47014e7f624174755554386d6a1611))

### Other

- Moves the Rust package to the repo root for simplicty ([d2aa618](https://github.com/pacharanero/sct/commit/d2aa6183e4299127a11e7c0d2beadab665e58aba))

- Extensive reqvie and update of docs ([e2755b1](https://github.com/pacharanero/sct/commit/e2755b12897bc3c8d35b8265a6fd00f998d2294c))

- Add SNOMED CT directories to .gitignore ([ed35d05](https://github.com/pacharanero/sct/commit/ed35d059808ef58404ae118235d010f2f110374d))

- Update mkdocs.yml to enhance navigation and add code copy feature ([e9db6e6](https://github.com/pacharanero/sct/commit/e9db6e6462ec3dd551fb81213061c0881a836bad))

- Update README.md for improved formatting and add trademark information ([cd248e0](https://github.com/pacharanero/sct/commit/cd248e00c348791ebcd6dc45e54c50f6b6c3876d))

- Remove video for now ([401bfb8](https://github.com/pacharanero/sct/commit/401bfb880c34e61f24b0def357a414f52075bcbf))

- Semantic search ([ef45de5](https://github.com/pacharanero/sct/commit/ef45de58a21f7db7331d548bda0c9a0b9ec3d3e5))

- Update documentation and improve SQL query examples across multiple files ([4866682](https://github.com/pacharanero/sct/commit/48666827e90dd8f3dc57fbe41958580db686e1f3))

- Walkthrough additions ([4fc60c4](https://github.com/pacharanero/sct/commit/4fc60c4d17d306ead54049e147dfd0f5bc973414))

- Specced out the next few roadmap items ([eba36ab](https://github.com/pacharanero/sct/commit/eba36ab7f89563d3aa4b6edb90f3d22054abb9b5))

- Initial tests, done in a weird way. a better way is incoming ([a24a866](https://github.com/pacharanero/sct/commit/a24a866b011e37ddcf0eb2afa6d06b20777589d7))

- Adds Transitive Closure Table feature which optimises some queries (by a small amount) ([1a0c8e4](https://github.com/pacharanero/sct/commit/1a0c8e43924ce50a18c16553dfdaad7c92abd2e9))

- Adds improved docs for Transitive Closure Tables, and some specs for new planned features ([3aa23bd](https://github.com/pacharanero/sct/commit/3aa23bdf38dbb118922af982a1c032cc29281da0))

- Devcontainer ([2bdb3fe](https://github.com/pacharanero/sct/commit/2bdb3fedaa32b2eabe938b3b93201b78a8014d52))

- Update DuckDB installation to version 1.5.1 ([38299b1](https://github.com/pacharanero/sct/commit/38299b1bd901216c112668c27ac153bedf0ef344))

- Refactor Dockerfile for better layer mgmt ([6d08674](https://github.com/pacharanero/sct/commit/6d08674fac560b377d4483bb9291acb86802b76a))

- Add jq, grep & ripgrep ([55ecda2](https://github.com/pacharanero/sct/commit/55ecda2cad10408e521afd74aff00f54a4129f63))

- Add development section to README with devcontainer setup instructions ([d5af93a](https://github.com/pacharanero/sct/commit/d5af93a70d3e84ad88d5d66a99e10079390a6147))

- Adds python3 and ollama ([2136971](https://github.com/pacharanero/sct/commit/21369715960d59672ba42aff52ed021d19584d5a))

- Merge pull request #11 from jonnyry/main

Devcontainer ([1f00cd1](https://github.com/pacharanero/sct/commit/1f00cd100d54c012322288d983711dd232cebf8f))

- Add FUNDING.yml to support GitHub Sponsors ([df49e39](https://github.com/pacharanero/sct/commit/df49e3926fbd37c859ae037b98b29237f64817dd))

- Fix inaccurate description of Hermes

Hermes is a library first, not a server — the HTTP and MCP interfaces
are optional wrappers. It can be embedded in-process with no network
overhead. It runs from source code or via Homebrew, not only as a JAR.
LMDB is BSD-licensed, not proprietary. Acknowledge the trade-offs: no
SQLite ad-hoc queryability, JVM startup cost — but note the runtime
performance. ([d64d951](https://github.com/pacharanero/sct/commit/d64d9511943b036835a6fa46aa81909e60418e95))

- Add example of installing UK monolith edition ([37724df](https://github.com/pacharanero/sct/commit/37724df82cd4d0f79f6a5796ebe2dc52cd59c5a3))

- Note Clojure as a potential barrier to contributions ([24f2cd3](https://github.com/pacharanero/sct/commit/24f2cd3a278afa87329f9ce2596b9ddf76f37719))

- Update why-build-this.md ([371550d](https://github.com/pacharanero/sct/commit/371550db341b6423f89006ce140741a171bba6a4))

- Merge pull request #12 from wardle/fix/hermes-description

Fix inaccurate description of Hermes ([bdbdfec](https://github.com/pacharanero/sct/commit/bdbdfec0655797a437aed10a778a95899e5dd6e8))

- Merge remote-tracking branch 'origin/main' ([e138723](https://github.com/pacharanero/sct/commit/e1387230bf7dd6ba8a8a8f5f9a0f60814a4a8cb1))

- TRUD-automation ([#9](https://github.com/pacharanero/sct/issues/9)) ([db6181a](https://github.com/pacharanero/sct/commit/db6181a9c9fbbfcaf786d1a105ee4a055eb6dc66))

- Sct trud check: verify local file SHA-256 against TRUD metadata

Previously `sct trud check` only checked whether a file with the expected
name existed in the releases directory — a corrupt or half-downloaded
local file would be reported as "up to date". Now, when the file is
present, its SHA-256 is re-computed and compared against the TRUD
metadata. On match, the verification is stated explicitly in the output.
On mismatch, the check exits 2 (same as "new release available") so a
`check && download` shell loop will heal the corruption automatically.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com> ([df7bcf2](https://github.com/pacharanero/sct/commit/df7bcf2108405927d5d7ca5c03ce3c3c97d52bc4))

- Minor tweaks to ETHICS.md ([386e158](https://github.com/pacharanero/sct/commit/386e1588e068f1828be4be367d66c7d87cc8b909))

- Split src into lib + bin so integration tests can live under tests/

Adds src/lib.rs that re-exports the existing modules (builder, commands,
rf2, schema) as pub, and strips the matching `mod` declarations from
src/main.rs. The binary now depends on the library via `use sct_rs::...`,
enabling integration tests under tests/ and downstream reuse, without
changing the visibility or semantics of any item inside the modules.

This is pass 1 of a two-pass refactor; pass 2 will migrate the tests
that exercise public/CLI behaviour into tests/ as integration tests
while leaving private-function unit tests in their #[cfg(test)] modules.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com> ([100c36c](https://github.com/pacharanero/sct/commit/100c36cc0da62c89822acf27d00c8aa031ec72ec))

- Bump version to 0.3.8 ([c4ec73d](https://github.com/pacharanero/sct/commit/c4ec73dd4a47a2f6f85023f5477d0531da26ff7d))

## [0.3.7] - 2026-04-02

### Other

- Add `sct lookup` command for direct SCTID and CTV3 code lookup

New CLI command that accepts a bare SCTID (numeric) to retrieve full
concept details, or a CTV3 code to reverse-lookup mapped SNOMED concepts
via the concept_maps table. Includes human-readable and --json output.

https://claude.ai/code/session_01NtVedfRo5Jpdh5TJ2dzxjE ([3db1f83](https://github.com/pacharanero/sct/commit/3db1f8319371605116fe29e13cce7ccadf9c3d98))

- Fix formatting in lookup command ([bee7455](https://github.com/pacharanero/sct/commit/bee74557707f363d1b595f84063b3bef84db3bd1))

- Merge pull request #8 from pacharanero/claude/add-snomed-lookup-eJU9D

Add `sct lookup` command for direct SCTID and CTV3 code lookup ([698b774](https://github.com/pacharanero/sct/commit/698b774caf13e74410e49ced640dd881ee4bc00a))

- Add sct serve FHIR R4 terminology server spec and roadmap ([ffb1ba2](https://github.com/pacharanero/sct/commit/ffb1ba2b36bb63a387f5e47557decc014ba37aba))

- Merge pull request #7 from pacharanero/claude/fhir-terminology-server-spec-TpT8x

spec: add sct serve FHIR R4 terminology server spec and roadmap ([c58f090](https://github.com/pacharanero/sct/commit/c58f0900afb025bc71cd09102be59d7430b971cf))

- Bump version to 0.3.7 ([e20d2d7](https://github.com/pacharanero/sct/commit/e20d2d7589d1fcb723456b5d5b12aca5071d5763))

## [0.3.5] - 2026-03-31

### Other

- Adds a Remotion video walkthrough ([13d5d56](https://github.com/pacharanero/sct/commit/13d5d56f0d8b79b0983993bb4e2d1bd0005d6acd))

- MCP gets some codelist tools ([046ddbf](https://github.com/pacharanero/sct/commit/046ddbf097489f8e472e4113f7ed89f8bb2a505c))

- Rename the crate name to sct-rs because sct is taken. Binary and usage stays the same. ([1e8a8c0](https://github.com/pacharanero/sct/commit/1e8a8c05d5a5eb18fd3a905f0d12c0a55a2e5ea6))

- Fmt ([174c81c](https://github.com/pacharanero/sct/commit/174c81c4c0d94a820a9432edf1e9b64c93781152))

- Linter appeasement ([a21ca99](https://github.com/pacharanero/sct/commit/a21ca99f6937664ca477aed4112a8c099c1e7032))

- Bump version to 0.3.5 ([a3e3562](https://github.com/pacharanero/sct/commit/a3e3562fa55c88d15f95047fa2149a68bc732d6b))

### Refactor

- Refactor and tidy of docs/specs ([bad1565](https://github.com/pacharanero/sct/commit/bad156551504e22e1188cfabfc647bbce349b645))

## [0.3.4] - 2026-03-31

### Other

- Add feature selection to install script for customizable cargo installation ([241bd83](https://github.com/pacharanero/sct/commit/241bd8354a3536e25680ccd9e8143aa61e247ab3))

- Improved documentation ([58f420a](https://github.com/pacharanero/sct/commit/58f420a36fb93ee2e7fbb1fb847d4afe637ddf08))

- Addition of codelists feature and improved documentation ([23cf791](https://github.com/pacharanero/sct/commit/23cf791fcbd6db0cd913c6609505f067d02ac052))

- Bump version to 0.3.4 ([211fb11](https://github.com/pacharanero/sct/commit/211fb11bebd0d7bbdd2b856d0c6200021479234a))

## [0.3.3] - 2026-03-30

### Chores

- Update Cargo.lock after dependency resolution ([f902350](https://github.com/pacharanero/sct/commit/f902350f9ef01db4a7e50f607f4788252239f9c0))

### Features

- **gui**: Add D3.js neighbourhood graph visualisation ([44f6f7e](https://github.com/pacharanero/sct/commit/44f6f7e93b1f98cf05ac179136b182cb13612c3d))

### Other

- Merge remote-tracking branch 'origin/claude/snomed-ct-graph-visualization-kVr5V' ([4348788](https://github.com/pacharanero/sct/commit/4348788f293d7a7d4ee87103d74198531c107460))

- Bump version to 0.3.3 ([9e30e5e](https://github.com/pacharanero/sct/commit/9e30e5ef4c5f8d8e87dacd7b5d8cec0b9cd2ea04))

## [0.3.2] - 2026-03-30

### Features

- CTV3 and Read v2 cross-map support from UK RF2 release files ([4c06635](https://github.com/pacharanero/sct/commit/4c066351d1512b2baaaaa95648e408fcce90aa11))

### Other

- Merge remote-tracking branch 'origin/claude/ctv3-snomed-mappings-vTrKY' ([34dd0bd](https://github.com/pacharanero/sct/commit/34dd0bddeac2e2d05e976989ccd710e1ad79c611))

- Adds CTV3 cross mapping capability and improved docs ([a9ea59a](https://github.com/pacharanero/sct/commit/a9ea59a4ebbee3f259c3b8e26c08cf33eebcab01))

- Bump version to 0.3.2 ([ac28de4](https://github.com/pacharanero/sct/commit/ac28de4bf5017614e283283535c487e5bce24818))

## [0.3.1] - 2026-03-30

### Chores

- Add CODE-OF-CONDUCT.md, CONTRIBUTING.md, and ETHICS.md files ([bfb1db6](https://github.com/pacharanero/sct/commit/bfb1db634c37ca513d616eadedca45dfa857058f))

### Features

- Set up GitHub Pages deployment and add Zola configuration ([681ebc7](https://github.com/pacharanero/sct/commit/681ebc755003a9f22e71e56167383830b5f09f76))

### Other

- Add docs/walkthrough.md feature tour and maintenance policy

- docs/walkthrough.md: 15-section hands-on tour of all sct features,
  structured as numbered demo scenes for the Remotion demo. Covers
  ndjson → sqlite → parquet → markdown → embed → semantic → mcp →
  tui/gui → diff/info/completions, with command examples, timings,
  and a UK layered-build section.
- specs/spec.md: adds a "Documentation maintenance" section specifying
  when walkthrough.md must be updated and noting its role as Remotion
  demo source material.

https://claude.ai/code/session_0173FhypUtGpoaAs5jrWezbv ([f59f3e7](https://github.com/pacharanero/sct/commit/f59f3e7a574964b85ed0f0b094a351f9caae7a08))

- Merge pull request #5 from pacharanero/claude/create-walkthrough-docs-BFclx

Add comprehensive walkthrough documentation and spec updates ([78168ab](https://github.com/pacharanero/sct/commit/78168ab34a24d132a093d958effe67bdd8417de5))

- Add docs/vector-search-landscape.md: SNOMED vector search ecosystem review

Comprehensive landscape of existing SNOMED CT tooling with vector/semantic
search capability: production servers (Snowstorm, Hermes, Ontoserver, JSL),
research embedding models (SapBERT, CODER, HiT, SNOBERT), clinical NLP tools
(MedCAT v2, scispaCy), RAG prototypes, and downloadable pre-computed
embeddings. Includes a capability comparison table and implications for sct,
notably the recommendation to support SapBERT as an embedding model option.

https://claude.ai/code/session_0173FhypUtGpoaAs5jrWezbv ([07a7ad9](https://github.com/pacharanero/sct/commit/07a7ad9ea01f25deb7343afa1d58a0cbf387d4d7))

- Merge remote-tracking branch 'origin/claude/create-walkthrough-docs-BFclx' ([6ae5cf9](https://github.com/pacharanero/sct/commit/6ae5cf943f37f7a5ebebaa8c02b0bfc8f266eae0))

- Switch docs site to Zensical ([ab3f9b6](https://github.com/pacharanero/sct/commit/ab3f9b65e247bc81d6c12d3265e2183c2e69b9a8))

- Bump version to 0.3.1 ([521d1b3](https://github.com/pacharanero/sct/commit/521d1b3162c62b5fb9b2050d8d80ddde34b4218f))

## [0.3.0] - 2026-03-28

### Bug fixes

- **mcp**: Echo client protocol version to fix 30s timeout with Claude Code 2.x ([dd91219](https://github.com/pacharanero/sct/commit/dd91219d088e9beedc839eff434ff9acb833b4a7))

- **mcp**: Support newline-delimited JSON transport (MCP spec 2025-11-25) ([6b9b52a](https://github.com/pacharanero/sct/commit/6b9b52a6e6b8f0e964dcb225cdbe199a063dea6d))

- Bench suite — global propagation and printf format string bugs ([55db6f4](https://github.com/pacharanero/sct/commit/55db6f454bd95605383283a918e610b03c759842))

- **gui**: Join concepts table to get hierarchy column in search ([07bace3](https://github.com/pacharanero/sct/commit/07bace39722ad08f0fad5cddc39f854c90293641))

### CI

- Update actions/checkout v4→v6, actions/cache v4→v5 ([bc9fd60](https://github.com/pacharanero/sct/commit/bc9fd6077854e5e237dc15d2e10a9a5e26307bd4))

- Add release workflow for Linux x86_64, macOS arm64/x86_64 binaries ([84b7ea5](https://github.com/pacharanero/sct/commit/84b7ea59191b973e0b0faf4a71d88b494dd786b5))

### Chores

- CI, docs, roadmap, queries ([9e443bd](https://github.com/pacharanero/sct/commit/9e443bda5913b68817e3feed227d4ed87ff582af))

- Update roadmap to reflect completed milestones, delete resolved queries ([82aa837](https://github.com/pacharanero/sct/commit/82aa83773162c40d934bc0be8f7af4031da4eaf3))

- Add *.db to .gitignore to exclude database files from version control ([c2502f5](https://github.com/pacharanero/sct/commit/c2502f57f9680ba96976b46df7370332c9a28800))

- Trim roadmap to outstanding work only, add next steps ([9aca79f](https://github.com/pacharanero/sct/commit/9aca79f0054f554e6c7bc6737db1229bfea37948))

- Add *.arrow to .gitignore to exclude Arrow files from version control ([6d517ec](https://github.com/pacharanero/sct/commit/6d517ec37575f2d89d4a4c4a3b70927edf952a0f))

- Upgrade all deps to latest stable ([bfe1aba](https://github.com/pacharanero/sct/commit/bfe1abaed565c9bafeda08692a1279d2a1db9222))

- Add *.db-shm and *.db-wal to gitignore ([12fa3ba](https://github.com/pacharanero/sct/commit/12fa3ba4378f9ad8cb68555cec0af4875d4228c9))

- Crates.io publishing, roadmap updates, licence ([ff2ec18](https://github.com/pacharanero/sct/commit/ff2ec1855f125416948f2ae30f435203ef40446a))

- Update GitHub Actions to latest stable versions ([a315738](https://github.com/pacharanero/sct/commit/a3157384efebc6c2f8a1a1e7f6aff3789eee3752))

### Documentation

- Add docs/ per subcommand, README as index ([3704497](https://github.com/pacharanero/sct/commit/370449713685e17f1cdcb13e94e6d5c206d2fe19))

- Sync spec.md with codebase; README subcommands as TOC ([837ec18](https://github.com/pacharanero/sct/commit/837ec1843672263effb3c4fa96c82f6669c3d1b2))

- Fix formatting in mcp.md and sqlite.md for consistency ([f9b4a59](https://github.com/pacharanero/sct/commit/f9b4a59f07692e9aa8cb81f26cfccecbc5fd560c))

- Address docs-improvements.md review points ([68d07e7](https://github.com/pacharanero/sct/commit/68d07e7429d023b05fb9e0fd456da68a5d9af138))

- Add tui and gui usage documentation ([f359729](https://github.com/pacharanero/sct/commit/f35972904cc29517a07dd4fbdd1f06f2a8ab93cd))

- Add completions subcommand documentation ([38fe3b2](https://github.com/pacharanero/sct/commit/38fe3b252fda3f9ba9f8e3f836070f2e39617219))

- **ndjson**: Update to reflect .zip input support ([cf41ae8](https://github.com/pacharanero/sct/commit/cf41ae828117317b244e05cfe892ac0908b63371))

### Features

- Add subcommand architecture + sqlite/parquet/markdown/mcp commands ([fee6517](https://github.com/pacharanero/sct/commit/fee65176a462561fb64115090efc468fbbd8e7eb))

- Add CI workflow and dependabot configuration for GitHub Actions ([51bb069](https://github.com/pacharanero/sct/commit/51bb0694c5a25eb906737e9caca38bddefbf4525))

- **markdown**: Add --mode hierarchy flag for one-file-per-hierarchy output ([b4dffde](https://github.com/pacharanero/sct/commit/b4dffdeca167ef844cd3ed1c7694a323b5070cdd))

- **mcp**: Validate schema_version on startup, warn or refuse if DB is too new ([32d31eb](https://github.com/pacharanero/sct/commit/32d31eb8217d5c7b7fe11936a5183e4fdb68d35b))

- **embed**: Implement Ollama HTTP embeddings with Arrow IPC output ([e2f8efc](https://github.com/pacharanero/sct/commit/e2f8efca70371aef0c8b8f334cd786142df30bbb))

- Add sct info and sct diff subcommands ([0e8083e](https://github.com/pacharanero/sct/commit/0e8083e3fd86ce5bd10c4fcfca5695643f7b46d1))

- Add sct lexical and sct semantic search commands ([bf602dd](https://github.com/pacharanero/sct/commit/bf602dd718208b7d9714e20db1311dddd923f6c8))

- Add shell completions (bash, zsh, fish, powershell, elvish) ([ae9793e](https://github.com/pacharanero/sct/commit/ae9793eff67ba55dd6d44c680558968bdfa2d1ee))

- Bench/ suite — automated local vs FHIR terminology server benchmarking ([ffd000b](https://github.com/pacharanero/sct/commit/ffd000b3aa25d299cd1c75e83ebd19c0a9544612))

- Timestamped benchmark filenames in benchmarks/ directory ([8325f82](https://github.com/pacharanero/sct/commit/8325f8293a762bbbc9d7573af631e474a2f984b6))

- Microsecond resolution + environment section at top of benchmark files ([bd2c99d](https://github.com/pacharanero/sct/commit/bd2c99da49652d165a440d2399c7609f83b4e8e7))

- Add sct tui and sct gui interactive exploration interfaces ([f0e8b11](https://github.com/pacharanero/sct/commit/f0e8b110aa7b44ce4cad1937b6b9e2cd4a84d5d2))

- **gui**: Add support for serving HTML from a specified file in development mode ([46f2535](https://github.com/pacharanero/sct/commit/46f253511f2c4e322208de524c1c1f2403da85ee))

- **ndjson**: Auto-extract RF2 .zip archives ([8e1efaa](https://github.com/pacharanero/sct/commit/8e1efaaf9b5dd1bf21424eacd2281d3374873d25))

### Other

- Initial commit: sct Layer 1 RF2-to-NDJSON converter ([b9481d3](https://github.com/pacharanero/sct/commit/b9481d3fbe0983916ff1a7e80a75903d376f58dc))

- Remove redundant sct/.gitignore ([40cf4b3](https://github.com/pacharanero/sct/commit/40cf4b39facb6e433d6e8190b9abc35b422ed9a8))

- Add Quick Start section to README ([05db422](https://github.com/pacharanero/sct/commit/05db4222b177d29cb052ecb537b7227b1894e6e2))

- Add .gitignore for downloads and update main .gitignore ([64b1c17](https://github.com/pacharanero/sct/commit/64b1c1786064fdabba14a84602b395786fc13b4a))

- Update clone URL in README for correct repository reference ([7aaf0f5](https://github.com/pacharanero/sct/commit/7aaf0f5ddac14631f1f9dea9cf88dbf8e2566726))

- Comprehensive specs ([757e98c](https://github.com/pacharanero/sct/commit/757e98cbc5e52ab050486ac687bbbc88467820c4))

- Benchmarks and reasoning ([e57607f](https://github.com/pacharanero/sct/commit/e57607fa75d43ea14e6a0848b514dca7c234927e))

- Remove outdated documentation files: BENCHMARKS.md, roadmap.md, spec.md, and why-build-this.md ([8e6c5cf](https://github.com/pacharanero/sct/commit/8e6c5cf9fba8bf025c1c33ce7592f5f7b6d4447b))

- Bump version ([b5180ed](https://github.com/pacharanero/sct/commit/b5180edacde9f7e3244341ec7b277d7e3dde08e2))

### Refactor

- Clean up formatting and improve readability in multiple command files ([b138d9c](https://github.com/pacharanero/sct/commit/b138d9cc4db285c420f54a1908a53d8552e7bae1))

### Tests

- Unit tests for rf2 parsing and builder logic (19 tests) ([dc9948c](https://github.com/pacharanero/sct/commit/dc9948c14612d54f6cb6a66aa61eb5a7201f4c04))


