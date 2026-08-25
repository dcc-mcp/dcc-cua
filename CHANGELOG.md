# Changelog

## [1.5.6](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.5...v1.5.6) (2026-08-25)


### Bug Fixes

* honor task grants for ordinary pointer input ([#207](https://github.com/dcc-mcp/dcc-cua/issues/207)) ([f00820f](https://github.com/dcc-mcp/dcc-cua/commit/f00820fe8cf569dda534fba33a03d08b2d957edf))

## [1.5.5](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.4...v1.5.5) (2026-08-25)


### Bug Fixes

* skip redundant navigation confirmations ([806dd0e](https://github.com/dcc-mcp/dcc-cua/commit/806dd0ef9b427d61193d13d111a42ad8139dfb57))

## [1.5.4](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.3...v1.5.4) (2026-08-24)


### Bug Fixes

* extend bounded UIA worker startup ([#204](https://github.com/dcc-mcp/dcc-cua/issues/204)) ([4d46c5b](https://github.com/dcc-mcp/dcc-cua/commit/4d46c5b50bbda73cced1315d288ab270a43a52a9))
* serialize Windows UIA fixtures ([#202](https://github.com/dcc-mcp/dcc-cua/issues/202)) ([ec74a4c](https://github.com/dcc-mcp/dcc-cua/commit/ec74a4c399ceaf6ed4ae497da03e7198bf1fa2e1))

## [1.5.3](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.2...v1.5.3) (2026-08-24)


### Bug Fixes

* retain semantic browser page origin ([#199](https://github.com/dcc-mcp/dcc-cua/issues/199)) ([9899a2f](https://github.com/dcc-mcp/dcc-cua/commit/9899a2f6c6259ba6f722a8ed3b2b6389d34f00f6))

## [1.5.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.1...v1.5.2) (2026-08-24)


### Bug Fixes

* bind browser task runtime sessions ([#196](https://github.com/dcc-mcp/dcc-cua/issues/196)) ([c520ad4](https://github.com/dcc-mcp/dcc-cua/commit/c520ad47469558c68aa184e32aa6bfb073ef9985))

## [1.5.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.0...v1.5.1) (2026-08-24)


### Bug Fixes

* accept namespaced browser task sessions ([#194](https://github.com/dcc-mcp/dcc-cua/issues/194)) ([9407b36](https://github.com/dcc-mcp/dcc-cua/commit/9407b36e22573b513504985d90e286fd50bc9966))
* preserve native latest release ([#189](https://github.com/dcc-mcp/dcc-cua/issues/189)) ([ae26331](https://github.com/dcc-mcp/dcc-cua/commit/ae26331840a9e0c27d2bc67f4f2c169e1126a417))
* publish Intel macOS release assets ([#192](https://github.com/dcc-mcp/dcc-cua/issues/192)) ([f81be0a](https://github.com/dcc-mcp/dcc-cua/commit/f81be0addfb5482ef05c47db9eae4bec40a5a447))

## [1.5.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.4.0...v1.5.0) (2026-08-24)


### Features

* add browser store credential preflight ([8fc6f9e](https://github.com/dcc-mcp/dcc-cua/commit/8fc6f9ed90dc98653990ee96dbd2832bf19e0492))
* authorize sensitive actions with user confirmation ([74a5b39](https://github.com/dcc-mcp/dcc-cua/commit/74a5b393ede268e139b3de70d2be6883c0eb1f9d))
* automate browser store publishing ([9c777e3](https://github.com/dcc-mcp/dcc-cua/commit/9c777e32540d8bdb47ac012f12776183de911b8c))
* keep credentials outside host ipc ([84b6144](https://github.com/dcc-mcp/dcc-cua/commit/84b6144691e33f88bacc1c3ec27d4cacbd66726a))
* land task-scoped user authorization ([7d1fbf2](https://github.com/dcc-mcp/dcc-cua/commit/7d1fbf216fab2c8a1a48079d99b16a8abbba231e))


### Bug Fixes

* bind browser prepare to task authorization ([ac417b3](https://github.com/dcc-mcp/dcc-cua/commit/ac417b35156d7918346f95b7c7d00e01ed7de2b3))
* close task authorization action schema ([245d41b](https://github.com/dcc-mcp/dcc-cua/commit/245d41b80af2063e46023c73f7451fb69e104816))
* disambiguate repeated browser consent ([08c47ac](https://github.com/dcc-mcp/dcc-cua/commit/08c47ac45aba08bc562691074c3fc1275c3c7bd6))
* improve CLI version and update checks ([f0bd6cb](https://github.com/dcc-mcp/dcc-cua/commit/f0bd6cbdb8979f308537a1d6fb49f31071f164cc))
* keep host-jsonl alive after output errors ([c035e07](https://github.com/dcc-mcp/dcc-cua/commit/c035e075ec4794f566bc29201706472b420f651c))
* support UIA expandable controls ([55e968b](https://github.com/dcc-mcp/dcc-cua/commit/55e968b09816f246391a7bf9cedfc00916df32e6))

## [1.4.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.3.3...v1.4.0) (2026-08-22)


### Features

* add persistent browser provider sessions ([2cd9919](https://github.com/dcc-mcp/dcc-cua/commit/2cd991993e8e278d4220589c6a33cb4d510188f3))
* extract reusable profile services ([#151](https://github.com/dcc-mcp/dcc-cua/issues/151)) ([cbf22e3](https://github.com/dcc-mcp/dcc-cua/commit/cbf22e3c578e8252dc4bfa052d7c38f9241b8037))


### Bug Fixes

* add typed computer use error contracts ([a5e0867](https://github.com/dcc-mcp/dcc-cua/commit/a5e0867dde9205b51ad5532a7cacb835455d30c6))
* align browser extension protocol contracts ([5b02594](https://github.com/dcc-mcp/dcc-cua/commit/5b02594ca1292ee795dca8837db9cdd43afe89b5))
* align safety lifecycle contracts ([#154](https://github.com/dcc-mcp/dcc-cua/issues/154)) ([a112433](https://github.com/dcc-mcp/dcc-cua/commit/a1124332bc186216b35190b7c1657553f197ffb4))
* bind exact captures to process identity ([d8b998a](https://github.com/dcc-mcp/dcc-cua/commit/d8b998ae2e77b2e2f41c1a9f6d4036fb912476c2))
* centralize sensitive application policy ([26f0c0b](https://github.com/dcc-mcp/dcc-cua/commit/26f0c0bbf666dac607638d2eef7416f6cd4f3229))
* consume equals-form cli flags consistently ([2b12709](https://github.com/dcc-mcp/dcc-cua/commit/2b1270917c31793c2eef4b90d0da81997d2edde6))
* eliminate error message control flow ([899d17f](https://github.com/dcc-mcp/dcc-cua/commit/899d17fb1d8b916064c500242bde3daeef3f1459))
* enforce desktop observation fencing in core ([5c9ccf5](https://github.com/dcc-mcp/dcc-cua/commit/5c9ccf5eba78060247565cae2c3d990772568cd0))
* enforce unscoped native tool policy ([f0d7c37](https://github.com/dcc-mcp/dcc-cua/commit/f0d7c371c699a9d64333f8cffa934995e709dd38))
* fail closed on invalid semantic profiles ([e6dadb3](https://github.com/dcc-mcp/dcc-cua/commit/e6dadb36fe1e621e40b2e077e67520838e661244))
* harden runtime lifecycle cleanup ([#153](https://github.com/dcc-mcp/dcc-cua/issues/153)) ([dd7cd4f](https://github.com/dcc-mcp/dcc-cua/commit/dd7cd4fe92ccde0a3aabd34fafadabf1559943f6))
* harden Windows named pipe identity ([09445d0](https://github.com/dcc-mcp/dcc-cua/commit/09445d0c720cec44f669b6388294a2f89cc32674))
* harden Windows UIA worker protocol ([c7dcd5e](https://github.com/dcc-mcp/dcc-cua/commit/c7dcd5e7b057f1fcd92699b691886ca2866963e5))
* honor hotkey modifiers across input routes ([92911c4](https://github.com/dcc-mcp/dcc-cua/commit/92911c432258cffdb4609139d20484d054a40dc1))
* make Escape hook lifecycle restartable ([4e922ea](https://github.com/dcc-mcp/dcc-cua/commit/4e922ea12c53f30c7971ce724fbc1e0ae7b472a6))
* preserve shared image handoffs ([4be1c83](https://github.com/dcc-mcp/dcc-cua/commit/4be1c839bfbac9c799a15e55ebc47e7e22522ea6))
* rebuild release PR from current main ([#157](https://github.com/dcc-mcp/dcc-cua/issues/157)) ([7734204](https://github.com/dcc-mcp/dcc-cua/commit/77342049e235eb04d04a2b1e03e3ff73e207d73a))
* satisfy cross-platform clippy ([69e75de](https://github.com/dcc-mcp/dcc-cua/commit/69e75de260606633292a2c06b4258bf4a9834f83))
* unpin browser extension releases ([22f0833](https://github.com/dcc-mcp/dcc-cua/commit/22f083367bff3fcc4ad8d09f72d0cf28d5bf2224))


### Performance Improvements

* bound observation image hot paths ([#155](https://github.com/dcc-mcp/dcc-cua/issues/155)) ([9a1f9ba](https://github.com/dcc-mcp/dcc-cua/commit/9a1f9ba5bdef207ee73a960ab3661ef7692bcc9d))

## [1.3.3](https://github.com/dcc-mcp/dcc-cua/compare/v1.3.2...v1.3.3) (2026-08-21)


### Bug Fixes

* **browser:** allow bounded semantic snapshot proofs ([58d1f93](https://github.com/dcc-mcp/dcc-cua/commit/58d1f93277edddd46769ba2dd7bc99d6d230c607))
* **browser:** allow existing-profile bind reconnect ([d2ef726](https://github.com/dcc-mcp/dcc-cua/commit/d2ef7269991e19e9a4dda07902edb07c3f3f0f99))
* **browser:** bound typed browser request deadlines ([d009004](https://github.com/dcc-mcp/dcc-cua/commit/d009004466a62c0716a8701d23ac568158789537))
* **browser:** prove existing-profile socket readiness ([a7f1307](https://github.com/dcc-mcp/dcc-cua/commit/a7f1307efdee8aac151062c2f2af8b3701fee74d))
* **browser:** scope repeated semantic actions exactly ([06c173b](https://github.com/dcc-mcp/dcc-cua/commit/06c173b0f62ddd460c1ce80edaf2e4e06e5ab51f))
* **window:** restore minimized bootstrap targets ([8e2f9d9](https://github.com/dcc-mcp/dcc-cua/commit/8e2f9d93e3a23a01a8aea98466c9fbd97d4974a9))

## [1.3.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.3.1...v1.3.2) (2026-08-20)


### Bug Fixes

* rebind browser evidence after session refresh ([6a91f19](https://github.com/dcc-mcp/dcc-cua/commit/6a91f19940c9d1108642258e9e875636f97650ae))
* repair release metadata contracts ([0c4095d](https://github.com/dcc-mcp/dcc-cua/commit/0c4095dc2bf821962cadeb11e94f37668c1fe969))
* restore Chrome existing-profile preparation ([90b491a](https://github.com/dcc-mcp/dcc-cua/commit/90b491a9180e9447d9580b8ac349091b4b32b7c5))

## [1.3.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.3.0...v1.3.1) (2026-08-16)


### Bug Fixes

* survive GitHub API rate-limit 403 in update ([2cf23cf](https://github.com/dcc-mcp/dcc-cua/commit/2cf23cfe5c4883e25699f2db39cf1e6f7cfff8ba))
* survive GitHub API rate-limit 403 in update ([1d0271b](https://github.com/dcc-mcp/dcc-cua/commit/1d0271bcf23c2bee367e3d9d3a621f0e55759f4f))

## [1.3.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.2.0...v1.3.0) (2026-08-16)


### Features

* add responsive banner skins ([fb1a6ff](https://github.com/dcc-mcp/dcc-cua/commit/fb1a6fff01748a6de32c3f9d7d80120ab5a4a176))
* add trusted action confirmation boundary ([f42ed75](https://github.com/dcc-mcp/dcc-cua/commit/f42ed75375c05ae7a6ef1649b6463c86036ed9a2))


### Bug Fixes

* advertise localized browser setup contract ([8256af9](https://github.com/dcc-mcp/dcc-cua/commit/8256af94442eef730e8ac54c7601a151aa2e456e))
* bound Windows application launch deadline ([4c180fa](https://github.com/dcc-mcp/dcc-cua/commit/4c180fa6884e8d4e21e6c4b8098b0924f1c9a091))
* fail closed on ambiguous window capture ([78f2b7e](https://github.com/dcc-mcp/dcc-cua/commit/78f2b7e4c69a3b1e95339b594857d2594b5c2d56))
* fail closed on invalid input dispatch ([#102](https://github.com/dcc-mcp/dcc-cua/issues/102)) ([5d5d683](https://github.com/dcc-mcp/dcc-cua/commit/5d5d683aa5c7af9e777f2a9453e66c9d7a565de7))
* pin foreground-proven browser setup ([9b98a83](https://github.com/dcc-mcp/dcc-cua/commit/9b98a83fdcf59aa33966afceac4912aa3ee36c96))
* pin hardened localized browser proof ([beb64c5](https://github.com/dcc-mcp/dcc-cua/commit/beb64c51772b4b0798a30a034f50aaade7a7e389))
* pin native browser tab invocation ([c72b25d](https://github.com/dcc-mcp/dcc-cua/commit/c72b25decc887b8e9441f145fafa2839710f02f2))
* pin pre-enabled browser endpoint proof ([9c67598](https://github.com/dcc-mcp/dcc-cua/commit/9c675989ed44f5e291b8ad430f26eda0c18c1f0c))
* preserve target frame during activation ([4351654](https://github.com/dcc-mcp/dcc-cua/commit/4351654ae318cd2ac2f1483cc16a0cb684ab06e0))
* satisfy host layout policy ([6f290b9](https://github.com/dcc-mcp/dcc-cua/commit/6f290b9f6af3fd5d2c6dd92e91ea9100a27e47a7))
* satisfy Windows capture test layout ([31b3f94](https://github.com/dcc-mcp/dcc-cua/commit/31b3f94a287d3f715e2075b0d7fa717773765044))
* support localized Chromium target proof ([1f7e93f](https://github.com/dcc-mcp/dcc-cua/commit/1f7e93f3d07eaae057b565316372348a61289847))

## [1.2.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.1.6...v1.2.0) (2026-08-13)


### Features

* add MCP image results for host JSONL ([d883758](https://github.com/dcc-mcp/dcc-cua/commit/d8837581994b780122ed21f9f8f293135876a815))


### Bug Fixes

* bound degraded window shutdown ([57ad8f8](https://github.com/dcc-mcp/dcc-cua/commit/57ad8f83951274473101092595812495e1faf9e7))
* tolerate delayed control banner startup ([95a8265](https://github.com/dcc-mcp/dcc-cua/commit/95a82651017b2c8c1f81dab22b2feea4a50fb4f2))

## [1.1.6](https://github.com/dcc-mcp/dcc-cua/compare/v1.1.5...v1.1.6) (2026-08-13)


### Bug Fixes

* preserve JSONL shared-memory snapshots ([#94](https://github.com/dcc-mcp/dcc-cua/issues/94)) ([9590d92](https://github.com/dcc-mcp/dcc-cua/commit/9590d9268dea4283e69f50b410ba2f55247d488a))

## [1.1.5](https://github.com/dcc-mcp/dcc-cua/compare/v1.1.4...v1.1.5) (2026-08-13)


### Bug Fixes

* normalize host keyboard action aliases ([cfb70e3](https://github.com/dcc-mcp/dcc-cua/commit/cfb70e3eb1eab38699520fc9e3d68ad9b9d15e0d))

## [1.1.4](https://github.com/dcc-mcp/dcc-cua/compare/v1.1.3...v1.1.4) (2026-08-13)


### Bug Fixes

* scale foreground input coordinates ([df9f806](https://github.com/dcc-mcp/dcc-cua/commit/df9f80697a82c5daa95a02dd130d58042108bdc0))

## [1.1.3](https://github.com/dcc-mcp/dcc-cua/compare/v1.1.2...v1.1.3) (2026-08-13)


### Bug Fixes

* normalize raw keyboard action aliases ([#86](https://github.com/dcc-mcp/dcc-cua/issues/86)) ([17d6ed7](https://github.com/dcc-mcp/dcc-cua/commit/17d6ed74ebda9a6495c5d22eec3a0260be43b9b0))
* preserve foreground cursor continuity ([e0f58a5](https://github.com/dcc-mcp/dcc-cua/commit/e0f58a5906680755e94bceffd420aef0bc2ccab4))

## [1.1.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.1.1...v1.1.2) (2026-08-13)


### Bug Fixes

* register foreground action routing ([f8924d7](https://github.com/dcc-mcp/dcc-cua/commit/f8924d79e30806985c5dc2fa57ddbd643e277d3a))

## [1.1.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.1.0...v1.1.1) (2026-08-13)


### Bug Fixes

* preserve local action continuity ([#80](https://github.com/dcc-mcp/dcc-cua/issues/80)) ([1381a04](https://github.com/dcc-mcp/dcc-cua/commit/1381a0420773ba8d13603edf969f6b91e8cd7154))

## [1.1.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.0.3...v1.1.0) (2026-08-12)


### Features

* add restore activate command ([#78](https://github.com/dcc-mcp/dcc-cua/issues/78)) ([5865b42](https://github.com/dcc-mcp/dcc-cua/commit/5865b4293e3b93a7565653d702f0ecfa9e28eb93))

## [1.0.3](https://github.com/dcc-mcp/dcc-cua/compare/v1.0.2...v1.0.3) (2026-08-12)


### Bug Fixes

* **cli:** preserve visible snapshot coordinate space ([#76](https://github.com/dcc-mcp/dcc-cua/issues/76)) ([e925bf3](https://github.com/dcc-mcp/dcc-cua/commit/e925bf30a73780840610da38352096fa7819f1b1))

## [1.0.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.0.1...v1.0.2) (2026-08-12)


### Bug Fixes

* honor input delivery and reject stale hosts ([#74](https://github.com/dcc-mcp/dcc-cua/issues/74)) ([70b9252](https://github.com/dcc-mcp/dcc-cua/commit/70b925247d42b796dad00a39cbe48e464b914e9e))

## [1.0.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.0.0...v1.0.1) (2026-08-12)


### Bug Fixes

* align install manifest identity ([#72](https://github.com/dcc-mcp/dcc-cua/issues/72)) ([493194e](https://github.com/dcc-mcp/dcc-cua/commit/493194e8a126e3c80e8b86342b3dfe3219174c8e))

## [1.0.0](https://github.com/dcc-mcp/dcc-cua/compare/v0.6.0...v1.0.0) (2026-08-12)


### ⚠ BREAKING CHANGES

* generalize profile packages and context ([#71](https://github.com/dcc-mcp/dcc-cua/issues/71))

### Features

* add profile startup context ([#69](https://github.com/dcc-mcp/dcc-cua/issues/69)) ([898941e](https://github.com/dcc-mcp/dcc-cua/commit/898941e8043fbf7c940aa7584a02843853810ea0))
* generalize profile packages and context ([#71](https://github.com/dcc-mcp/dcc-cua/issues/71)) ([d65b28e](https://github.com/dcc-mcp/dcc-cua/commit/d65b28e205c016ae6c0cc8608f83cc6888c3309c))

## [0.6.0](https://github.com/dcc-mcp/dcc-cua/compare/v0.5.3...v0.6.0) (2026-08-12)


### Features

* **host:** advertise multi-agent session concurrency ([6a6cfab](https://github.com/dcc-mcp/dcc-cua/commit/6a6cfab5a54e06e08f0fa0ebfde80859263d5ade))

## [0.5.3](https://github.com/dcc-mcp/dcc-cua/compare/v0.5.2...v0.5.3) (2026-08-12)


### Bug Fixes

* follow owned modal windows with explicit rebind ([#64](https://github.com/dcc-mcp/dcc-cua/issues/64)) ([8066f2e](https://github.com/dcc-mcp/dcc-cua/commit/8066f2e9fd57f36b9ed17d78ee20f035eaa28422))
* **windows:** synchronize foreground activation before input ([#66](https://github.com/dcc-mcp/dcc-cua/issues/66)) ([9e00a47](https://github.com/dcc-mcp/dcc-cua/commit/9e00a473c5f2fe210d0751b027b8028455ac445d))

## [0.5.2](https://github.com/dcc-mcp/dcc-cua/compare/v0.5.1...v0.5.2) (2026-08-12)


### Bug Fixes

* clarify Windows CUA fallback contracts ([7c35660](https://github.com/dcc-mcp/dcc-cua/commit/7c356600b00c871fc0635dfafd3f809431898afe))
* **windows:** activate exact windows across input threads ([#63](https://github.com/dcc-mcp/dcc-cua/issues/63)) ([936704a](https://github.com/dcc-mcp/dcc-cua/commit/936704ae93df42c871c314f313b1cd4d9b88e0ea))

## [0.5.1](https://github.com/dcc-mcp/dcc-cua/compare/v0.5.0...v0.5.1) (2026-08-12)


### Bug Fixes

* expose route-specific diagnostics readiness ([#56](https://github.com/dcc-mcp/dcc-cua/issues/56)) ([9b35f96](https://github.com/dcc-mcp/dcc-cua/commit/9b35f967f330310761d83f74bb3ffed19ae5bedd))

## [0.5.0](https://github.com/dcc-mcp/dcc-cua/compare/v0.4.0...v0.5.0) (2026-08-12)


### Features

* add shijie qunchao profile example ([b266740](https://github.com/dcc-mcp/dcc-cua/commit/b266740bacc7e34dc995e83dcd0eaa790786ffd6))


### Bug Fixes

* harden exact Windows UI recovery ([a44725c](https://github.com/dcc-mcp/dcc-cua/commit/a44725ce4065c44642eec75bd0479ff6809d40c3))
* keep held key input within layout limits ([b360f97](https://github.com/dcc-mcp/dcc-cua/commit/b360f97efd99eab9030e4b35fea0af914836fa05))
* make held game input interruptible ([09774f7](https://github.com/dcc-mcp/dcc-cua/commit/09774f734f703ea7c8fbeb1bffdc0b572133bbe6))
* recover read-only UIA snapshots ([#55](https://github.com/dcc-mcp/dcc-cua/issues/55)) ([88dc586](https://github.com/dcc-mcp/dcc-cua/commit/88dc5867ae967e5f71c9cf53c6361a25b7ce0def))
* release workspace-wide changes ([09445d2](https://github.com/dcc-mcp/dcc-cua/commit/09445d2538807b6c2e4adea37d3d4caa00ac5269))
* stabilize realtime game input and banner visibility ([56bb6de](https://github.com/dcc-mcp/dcc-cua/commit/56bb6dee30582ceb34590381d649c59309c7f1a2))
* support persistent game input control ([25105f4](https://github.com/dcc-mcp/dcc-cua/commit/25105f47c5a0614ce6865c282081d75b8935b91f))
* sync Windows workspace features ([e69ec2d](https://github.com/dcc-mcp/dcc-cua/commit/e69ec2d918cd247a23f1224f1208b9685f4183d0))
* use valid root release anchor ([#53](https://github.com/dcc-mcp/dcc-cua/issues/53)) ([e5db2e5](https://github.com/dcc-mcp/dcc-cua/commit/e5db2e5238b218b328c02e55905c91ea4972bc30))

## Changelog

All notable changes to `dcc-cua` are tracked here by release-please.
