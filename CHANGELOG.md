# Changelog

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
