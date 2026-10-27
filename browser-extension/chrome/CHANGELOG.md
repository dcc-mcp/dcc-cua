# Changelog

## [0.4.0](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.3.2...dcc-cua-browser-extension-v0.4.0) (2026-10-27)


### Features

* **extension:** add scoped same-origin tab navigation ([c106421](https://github.com/dcc-mcp/dcc-cua/commit/c1064212bb6b726ae9836837712b4345393edd24))


### Bug Fixes

* **extension:** retain providers after navigation preflight refusals ([b71aef0](https://github.com/dcc-mcp/dcc-cua/commit/b71aef0083aba08f2ab076453fc28991eaac42dd))

## [0.3.2](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.3.1...dcc-cua-browser-extension-v0.3.2) (2026-10-22)


### Bug Fixes

* bind extension pairing and input to authorized tasks ([ace0600](https://github.com/dcc-mcp/dcc-cua/commit/ace0600117840a2a31bb81e5050866eff77f6043))
* clear rejected native browser pairings ([1d31a5e](https://github.com/dcc-mcp/dcc-cua/commit/1d31a5e365d7d8cf7a3ca14f9f51ad94e1ba645b))
* **extension:** bind browser input to authorized tasks ([#365](https://github.com/dcc-mcp/dcc-cua/issues/365)) ([ef46bf3](https://github.com/dcc-mcp/dcc-cua/commit/ef46bf3fe66570b0012b2c211f968eb6ccd3d84a))
* integrate provider startup and disconnect cleanup ([6b95e3d](https://github.com/dcc-mcp/dcc-cua/commit/6b95e3ddb861be087cf7fb9368e452d7cb8106ed))
* integrate provider startup and disconnect cleanup ([0aa44d2](https://github.com/dcc-mcp/dcc-cua/commit/0aa44d2dd436229dd014bb80f89d3312abcdc9c4))
* reconcile Driver SDK upgrade with main ([d5242f5](https://github.com/dcc-mcp/dcc-cua/commit/d5242f5b13f8db1820387512e8acbe819f8acfef))

## [0.3.1](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.3.0...dcc-cua-browser-extension-v0.3.1) (2026-10-19)


### Bug Fixes

* **release:** bind draft sources before read-only builds ([b0038fc](https://github.com/dcc-mcp/dcc-cua/commit/b0038fc1d1be182e6a0f6dd7a596d9cb844cddce))

## [0.3.0](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.2.4...dcc-cua-browser-extension-v0.3.0) (2026-10-17)


### Features

* integrate exact-window capture and single executable delivery ([#330](https://github.com/dcc-mcp/dcc-cua/issues/330)) ([7a3bc05](https://github.com/dcc-mcp/dcc-cua/commit/7a3bc05d135f90ecd892924b941a6525aadbddc9))

## [0.2.4](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.2.3...dcc-cua-browser-extension-v0.2.4) (2026-10-16)


### Bug Fixes

* **ci:** use public npm tarballs for browser extension ([8c5782e](https://github.com/dcc-mcp/dcc-cua/commit/8c5782e29be7f8254f63aef7ac7748bc9afc9b1b))

## [0.2.3](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.2.2...dcc-cua-browser-extension-v0.2.3) (2026-10-13)


### Bug Fixes

* **cli:** add console-free MCP entry point ([#316](https://github.com/dcc-mcp/dcc-cua/issues/316)) ([4bde8f0](https://github.com/dcc-mcp/dcc-cua/commit/4bde8f072ba94c9ec7cf5f33d5bd3e81aab9c0a7))

## [0.2.2](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.2.1...dcc-cua-browser-extension-v0.2.2) (2026-09-13)


### Bug Fixes

* **cli:** enforce structured stream boundaries ([af8a5d6](https://github.com/dcc-mcp/dcc-cua/commit/af8a5d66858a4281b22ed1a51137edb1c24c79cb))
* contain private worker diagnostics ([b6d6681](https://github.com/dcc-mcp/dcc-cua/commit/b6d668199a3c3b0480c614c0be699c777c7aff78))
* harden CI artifact identity gates ([988e191](https://github.com/dcc-mcp/dcc-cua/commit/988e1918cc863332880298efe4769b4a52148e0a))
* remove redundant task authorization gate ([#261](https://github.com/dcc-mcp/dcc-cua/issues/261)) ([013ac8b](https://github.com/dcc-mcp/dcc-cua/commit/013ac8b18546631cb0c44032148aa0f421ab5c28))
* validate browser extension store artwork ([d7cabae](https://github.com/dcc-mcp/dcc-cua/commit/d7cabae88589b22220e4d90f829652fc35fbb11e))


### Performance Improvements

* bound browser semantic snapshot layout work ([#272](https://github.com/dcc-mcp/dcc-cua/issues/272)) ([20ccb39](https://github.com/dcc-mcp/dcc-cua/commit/20ccb399ad1f7c3ee8a76a42fe5ea4f419f1b948))
* cache browser bridge injection per document ([#271](https://github.com/dcc-mcp/dcc-cua/issues/271)) ([71c8052](https://github.com/dcc-mcp/dcc-cua/commit/71c805280fbb20e8eb3375b0ddaff8da18f1f7b8))
* cache browser pairing state ([#273](https://github.com/dcc-mcp/dcc-cua/issues/273)) ([c6263cd](https://github.com/dcc-mcp/dcc-cua/commit/c6263cd9eeddb662e3d3931917c4ba3a51acb854))

## [0.2.1](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.2.0...dcc-cua-browser-extension-v0.2.1) (2026-08-26)


### Bug Fixes

* grant release dispatch permission ([a53cb95](https://github.com/dcc-mcp/dcc-cua/commit/a53cb95dd4af307e3015de6345ee86bceeae48a6))
* harden release refresh contracts ([63b7ec8](https://github.com/dcc-mcp/dcc-cua/commit/63b7ec802f8c90f3f06074e3f2726bdedad039de))
* reject unsafe release archive paths ([012b616](https://github.com/dcc-mcp/dcc-cua/commit/012b616434f90f0c7be13c41a512e13a520019c7))
* validate extension assets before publish ([e302d69](https://github.com/dcc-mcp/dcc-cua/commit/e302d69267c53218686a19ccdab1a201339fa3a7))
* verify immutable release transport ([ebc5d40](https://github.com/dcc-mcp/dcc-cua/commit/ebc5d40539a512d2d9f82767b3b82202698dd66b))

## [0.2.0](https://github.com/dcc-mcp/dcc-cua/compare/dcc-cua-browser-extension-v0.1.0...dcc-cua-browser-extension-v0.2.0) (2026-08-24)


### Features

* add browser store credential preflight ([8fc6f9e](https://github.com/dcc-mcp/dcc-cua/commit/8fc6f9ed90dc98653990ee96dbd2832bf19e0492))
* automate browser store publishing ([9c777e3](https://github.com/dcc-mcp/dcc-cua/commit/9c777e32540d8bdb47ac012f12776183de911b8c))
* keep credentials outside host ipc ([84b6144](https://github.com/dcc-mcp/dcc-cua/commit/84b6144691e33f88bacc1c3ec27d4cacbd66726a))


### Bug Fixes

* align browser extension protocol contracts ([5b02594](https://github.com/dcc-mcp/dcc-cua/commit/5b02594ca1292ee795dca8837db9cdd43afe89b5))
* align safety lifecycle contracts ([#154](https://github.com/dcc-mcp/dcc-cua/issues/154)) ([a112433](https://github.com/dcc-mcp/dcc-cua/commit/a1124332bc186216b35190b7c1657553f197ffb4))
* unpin browser extension releases ([22f0833](https://github.com/dcc-mcp/dcc-cua/commit/22f083367bff3fcc4ad8d09f72d0cf28d5bf2224))

## 0.1.0 (2026-08-21)


### Features

* **browser-extension:** add WXT multi-browser provider ([576e9a5](https://github.com/dcc-mcp/dcc-cua/commit/576e9a54b47cd2aa62b44dce4fafb35284c4c838))

## Changelog

All notable changes to the DCC-CUA browser extension are documented here.

This component is versioned and released independently from the native
`dcc-cua` binary.
