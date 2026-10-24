# Changelog

## [1.14.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.13.0...v1.14.0) (2026-10-23)


### Features

* add guarded exact pixel dragging ([41731f2](https://github.com/dcc-mcp/dcc-cua/commit/41731f2fb208139e67345cc519168a66b113f162))
* add native focused popup diagnostics ([f541237](https://github.com/dcc-mcp/dcc-cua/commit/f5412374d40d9993a141460ab51cbda90e77fbe3))
* expose bound native popup diagnostics ([1b79fd7](https://github.com/dcc-mcp/dcc-cua/commit/1b79fd757c7dccf8b54bccf3ecc59f56d95e33b9))
* guard observed capture popup keyboard input ([ebe9f93](https://github.com/dcc-mcp/dcc-cua/commit/ebe9f93aac6053ef8ee47938dc5f5e71ec165c3a))
* observe selected native capture popups for keyboard input ([edac0a9](https://github.com/dcc-mcp/dcc-cua/commit/edac0a977aab79b39ec4c2acd9376fb02d5a8cf6))
* support bounded drags and observed Nuke search popups ([#374](https://github.com/dcc-mcp/dcc-cua/issues/374)) ([d61ba68](https://github.com/dcc-mcp/dcc-cua/commit/d61ba68d850ad9925cc3a28b08b127d68da74af8))


### Bug Fixes

* accept proven capture release during completed checks ([c23654f](https://github.com/dcc-mcp/dcc-cua/commit/c23654f668b31592d8dcb6b51652e363fc6792b0))
* attest popup keyboard ceiling in started tasks ([e7b1432](https://github.com/dcc-mcp/dcc-cua/commit/e7b14329c15be64c072ef08d448c08db7f43b4fc))
* await asynchronous registered cursor suppression ([df8de9c](https://github.com/dcc-mcp/dcc-cua/commit/df8de9ca1ff27b665c58bdfa978f09ebb6eb13d2))
* await balanced popup keyboard closure ([9249c31](https://github.com/dcc-mcp/dcc-cua/commit/9249c31f7e5d7e0fd81389c712627e35ebad9b7d))
* exclude registered overlays throughout native input ([42d413c](https://github.com/dcc-mcp/dcc-cua/commit/42d413c99dc0892f5445c4c6faa387f48d1619e4))
* gate capture on cursor restoration acknowledgements ([ac9c60e](https://github.com/dcc-mcp/dcc-cua/commit/ac9c60e873c5534fd7646fbb86a110749445e8c3))
* hide presenter before confirming cursor exclusion ([791d63c](https://github.com/dcc-mcp/dcc-cua/commit/791d63ca71c083d4c1fc828a11413f352b565da2))
* preserve peer overlays during native input ([1d98ae3](https://github.com/dcc-mcp/dcc-cua/commit/1d98ae319b18bb32e3c9ed6a2ee2d833c9eb4efa))
* recognize completed popup focus return to the exact root ([401bab8](https://github.com/dcc-mcp/dcc-cua/commit/401bab875a08eea92c9d1e46bae74440ece6db07))
* recognize forward popup closure in final keyboard checks ([ff01c57](https://github.com/dcc-mcp/dcc-cua/commit/ff01c57de9dc052899f77e27115fb58c3cf94c26))
* retain native input refusal diagnostics ([64dec14](https://github.com/dcc-mcp/dcc-cua/commit/64dec14d93a6258cf5a8edd3f90a341bdb4ccc08))
* retain native popup input refusal diagnostics ([a81f93d](https://github.com/dcc-mcp/dcc-cua/commit/a81f93d4ede542f84891506a128dac3f74a1bf62))
* serialize registered cursor restoration acknowledgements ([9a688bd](https://github.com/dcc-mcp/dcc-cua/commit/9a688bd5568ab4e1683e1a6bf6bfc73e56843bff))
* settle already destroyed popup postchecks ([f43a55f](https://github.com/dcc-mcp/dcc-cua/commit/f43a55fbcfd58efec761ffeb2fb621fd840e3f3f))
* validate popup captures by native capture mode ([46476bb](https://github.com/dcc-mcp/dcc-cua/commit/46476bb2e69597b9f5f03a1c9ad7f2e248c0bb4d))
* validate popup geometry after completed keyboard input ([3cc71ff](https://github.com/dcc-mcp/dcc-cua/commit/3cc71ffc079a165e512e85ddfee17c0d9d77f039))

## [1.13.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.12.2...v1.13.0) (2026-10-22)


### Features

* **browser:** pin trusted download filename metadata ([cbbceb6](https://github.com/dcc-mcp/dcc-cua/commit/cbbceb6a06d2e153f02a0391f1536ce0b166638b))
* **browser:** preserve download names and recover owned setup tabs ([#371](https://github.com/dcc-mcp/dcc-cua/issues/371)) ([e1a449e](https://github.com/dcc-mcp/dcc-cua/commit/e1a449e56cf2ba36ff32a74cde81178fdd023757))
* **browser:** request completed download filename metadata ([b272e6d](https://github.com/dcc-mcp/dcc-cua/commit/b272e6df08656be6bd4d1639ce7ae7c08fa31843))


### Bug Fixes

* attest macOS provider births and bound socket paths ([7144094](https://github.com/dcc-mcp/dcc-cua/commit/714409465285a5a1bc9047c7cea3f80dd1653122))
* await provider readiness and reconnect cleanup ([89cb203](https://github.com/dcc-mcp/dcc-cua/commit/89cb2038037fe61351edbf85e27b6cf9e9b31f1b))
* **browser:** clean up owned Windows setup tabs ([5c1e6a1](https://github.com/dcc-mcp/dcc-cua/commit/5c1e6a1b3b6a5eb520c433dbb194c14793c3c344))
* **browser:** report manual existing-profile setup recovery ([d412d37](https://github.com/dcc-mcp/dcc-cua/commit/d412d374c51f738bd99dc2d9312a71323379cf78))
* **ci:** separate GUI compilation from runtime timeout ([e764be8](https://github.com/dcc-mcp/dcc-cua/commit/e764be87f694a2f69f1415461036087ef5b31979))
* declare host endpoint test dependency ([7c11700](https://github.com/dcc-mcp/dcc-cua/commit/7c11700cb53081790caad0ded0095d4bf6631ffe))
* **extension:** bind browser input to authorized tasks ([#365](https://github.com/dcc-mcp/dcc-cua/issues/365)) ([ef46bf3](https://github.com/dcc-mcp/dcc-cua/commit/ef46bf3fe66570b0012b2c211f968eb6ccd3d84a))
* integrate native macOS pairing identity ([7e166d7](https://github.com/dcc-mcp/dcc-cua/commit/7e166d7c3be7c7f2a505b0d0c430c3c31d0e9cc3))
* integrate native macOS pairing identity ([a73ac44](https://github.com/dcc-mcp/dcc-cua/commit/a73ac44ba12d4141025989ba48d2eff2b5892bb6))
* integrate provider startup and disconnect cleanup ([6b95e3d](https://github.com/dcc-mcp/dcc-cua/commit/6b95e3ddb861be087cf7fb9368e452d7cb8106ed))
* integrate provider startup and disconnect cleanup ([0aa44d2](https://github.com/dcc-mcp/dcc-cua/commit/0aa44d2dd436229dd014bb80f89d3312abcdc9c4))
* reconcile Driver SDK upgrade with main ([d5242f5](https://github.com/dcc-mcp/dcc-cua/commit/d5242f5b13f8db1820387512e8acbe819f8acfef))
* update Driver SDK to 0.36.0 ([#356](https://github.com/dcc-mcp/dcc-cua/issues/356)) ([a9b3af1](https://github.com/dcc-mcp/dcc-cua/commit/a9b3af1f36c990ea25b9079d00356d055ed44fb2))

## [1.12.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.12.1...v1.12.2) (2026-10-21)


### Bug Fixes

* align capture boundary tests and recovery scopes ([e90beb4](https://github.com/dcc-mcp/dcc-cua/commit/e90beb461d5a3e150a65f30ce6ab393bc0667403))
* focus exact native windows within guarded input attachments ([065ea57](https://github.com/dcc-mcp/dcc-cua/commit/065ea57b0315b169df16ad23811f6907473d391e))
* integrate current capture behavior with foreground recovery ([4e68994](https://github.com/dcc-mcp/dcc-cua/commit/4e68994fca668bb5a0da50239debe6dac644339e))
* preserve activation frames in physical DPI coordinates ([709aa88](https://github.com/dcc-mcp/dcc-cua/commit/709aa880fdd5fcb0d037aa95c61e4898820676f6))
* recover explicit foreground activation with a fenced UIA root ([fa5bc8c](https://github.com/dcc-mcp/dcc-cua/commit/fa5bc8cccd05d9fdc6e0a7a31849efb82b8686d5))
* recover Windows activation and empty occluders ([b81808b](https://github.com/dcc-mcp/dcc-cua/commit/b81808be09b6520d72ad82a6d6539af345abfd14))
* recover Windows foreground and physical-frame validation ([#366](https://github.com/dcc-mcp/dcc-cua/issues/366)) ([52237c9](https://github.com/dcc-mcp/dcc-cua/commit/52237c9720a14a45036e817f5e882feaf76be705))
* reduce pixel snapshot encoding and guide window recovery ([4c70a65](https://github.com/dcc-mcp/dcc-cua/commit/4c70a65a079f84b1551a418b8e01b978611e3b34))
* reduce pixel snapshot encoding and guide window recovery ([54fe50b](https://github.com/dcc-mcp/dcc-cua/commit/54fe50b90c71fa2693b94083f64db1b393042ffe))
* separate pixel observation and input recovery ([9c10a71](https://github.com/dcc-mcp/dcc-cua/commit/9c10a71a99a67b22ed256375bf1ff35fa41dbb88))
* separate pixel observation and input recovery ([9c10a71](https://github.com/dcc-mcp/dcc-cua/commit/9c10a71a99a67b22ed256375bf1ff35fa41dbb88))
* separate pixel observation and input recovery ([5583b1f](https://github.com/dcc-mcp/dcc-cua/commit/5583b1fce6208e4db4520de2f110d39d4b6f590c))

## [1.12.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.12.0...v1.12.1) (2026-10-21)


### Bug Fixes

* admit read-only extension task diagnostics ([#360](https://github.com/dcc-mcp/dcc-cua/issues/360)) ([4f8613f](https://github.com/dcc-mcp/dcc-cua/commit/4f8613f184b998c9a61f03f6876047e9ba43181b))

## [1.12.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.11.2...v1.12.0) (2026-10-19)


### Features

* **browser:** enrich browser_download completion with proven artifact metadata ([8efbb7a](https://github.com/dcc-mcp/dcc-cua/commit/8efbb7a688bafebb02b5eae33538a24efe8b0337))
* **browser:** enrich browser_download completion with proven artifact metadata ([7831d89](https://github.com/dcc-mcp/dcc-cua/commit/7831d8987b2d09da1e8bca91a322e3d20be7716a))


### Bug Fixes

* **browser:** offload download enrichment off the async executor ([4849803](https://github.com/dcc-mcp/dcc-cua/commit/4849803e19f0989afbf7d428f208f4b0bfccd496))

## [1.11.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.11.1...v1.11.2) (2026-10-18)


### Bug Fixes

* **browser:** keep prepare continuation on the public task bridge ([442bc3a](https://github.com/dcc-mcp/dcc-cua/commit/442bc3ad0c8303062f46e4cc6500a758d0fd8184))
* clarify connection-scoped task and observation lifetimes ([8c5a961](https://github.com/dcc-mcp/dcc-cua/commit/8c5a9611bc66fde701c7235b3b0d8d25bd37ad39))
* explain connection-scoped task and observation lifetimes ([e4d5df9](https://github.com/dcc-mcp/dcc-cua/commit/e4d5df9014acee3111e4311a12b95f442f0472c3))
* make lifetime recovery guidance executable ([58601f4](https://github.com/dcc-mcp/dcc-cua/commit/58601f4b022a62a0d6141435c6a878a062067494))
* **release:** authenticate Latest metadata requests ([8268f9c](https://github.com/dcc-mcp/dcc-cua/commit/8268f9caed86f29026fcf8820872e6bd9cd5242a))
* **release:** authenticate Latest metadata requests ([42257f3](https://github.com/dcc-mcp/dcc-cua/commit/42257f3b069316aac4a6d7b73fe37a4f5b62add4))
* **release:** bind draft sources before read-only builds ([b0038fc](https://github.com/dcc-mcp/dcc-cua/commit/b0038fc1d1be182e6a0f6dd7a596d9cb844cddce))
* **release:** bind resume to original uploaded asset identities ([a50c01f](https://github.com/dcc-mcp/dcc-cua/commit/a50c01fed580e216a693d7becf32264c70975be0))
* **release:** read draft publication metadata by release ID ([e5312fc](https://github.com/dcc-mcp/dcc-cua/commit/e5312fc8512c988746bc0fde7df6a07076fc7af0))
* **release:** read original build logs with terminal escapes ([e5defa0](https://github.com/dcc-mcp/dcc-cua/commit/e5defa04fc18bb523b15a749de242eb5be7db344))
* **release:** read original build logs with terminal escapes ([072e065](https://github.com/dcc-mcp/dcc-cua/commit/072e065ab844c51c2833f2cc543be400766a715c))
* **release:** resolve draft metadata by release identity ([92b3065](https://github.com/dcc-mcp/dcc-cua/commit/92b3065a2bb7803798803e714be519fcdbe6ce61))
* **release:** resolve draft source bindings before read-only builds ([817bbc9](https://github.com/dcc-mcp/dcc-cua/commit/817bbc952ab198f3290cabde183b7061b65d636f))
* **release:** resume from verified public prerelease staging ([d3bd6c8](https://github.com/dcc-mcp/dcc-cua/commit/d3bd6c89609f9933e029d2e7117073cf13945fb4))
* **release:** resume publication from verified native artifacts ([6c3e0ff](https://github.com/dcc-mcp/dcc-cua/commit/6c3e0ffbeee32730077a6dd85506eaaceadc4f97))
* **release:** resume publication from verified native artifacts ([65f3b1d](https://github.com/dcc-mcp/dcc-cua/commit/65f3b1d54e840cd704a2d2c9e0f34145701267d4))

## [1.11.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.11.0...v1.11.1) (2026-10-18)


### Bug Fixes

* **cli:** distinguish absent resident hosts from other transport failures ([e4e4eae](https://github.com/dcc-mcp/dcc-cua/commit/e4e4eaec237ab1feae37552f59bb690d26c1bc15))
* **cli:** explain when the resident Host is not running ([6951545](https://github.com/dcc-mcp/dcc-cua/commit/6951545255de82da80a043d00e872cb04c38d1f2))
* preserve occlusion diagnostics and verify partial native capture ([6cc7cce](https://github.com/dcc-mcp/dcc-cua/commit/6cc7cce001289c7ea5b308e3b3cddb8badf7bf42))
* remove redundant unsafe capture proof block ([f099a5d](https://github.com/dcc-mcp/dcc-cua/commit/f099a5dd4a88851f27ba3ac5a9a7fb2ceda27bf7))
* retain scoped pixel observations under partial occlusion ([b6b05b4](https://github.com/dcc-mcp/dcc-cua/commit/b6b05b4c0aa3667052992a29590c8f198b1ffb08))
* retain scoped pixel observations under partial occlusion ([61c65ba](https://github.com/dcc-mcp/dcc-cua/commit/61c65ba0f4bd7e6083dd7229441f565acc04a1e4))

## [1.11.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.10.1...v1.11.0) (2026-10-17)


### Features

* add read-only local doctor context ([fdbf2bc](https://github.com/dcc-mcp/dcc-cua/commit/fdbf2bcad32a77a696d86cc2b1184f3420522b94))
* add read-only local doctor diagnostics ([9b25773](https://github.com/dcc-mcp/dcc-cua/commit/9b25773d731602779e90958ae5b0e5246aeb6ba3))
* integrate exact-window capture and single executable delivery ([#330](https://github.com/dcc-mcp/dcc-cua/issues/330)) ([7a3bc05](https://github.com/dcc-mcp/dcc-cua/commit/7a3bc05d135f90ecd892924b941a6525aadbddc9))


### Bug Fixes

* **cli:** preserve menu and diagnostics manifest contracts ([3168406](https://github.com/dcc-mcp/dcc-cua/commit/31684062329af7442c9c2c3e8441ef14451c2d3e))
* **host:** discard prepared pixels after authority ends ([d7a5507](https://github.com/dcc-mcp/dcc-cua/commit/d7a55079e4c0f66a557a9597834443e8b085ed35))
* **host:** drain every session with ended task authority ([301f2c0](https://github.com/dcc-mcp/dcc-cua/commit/301f2c08bd20b1caa88f2a6fab7493641d199772))
* **host:** preserve task authorization errors after cleanup ([89ff00e](https://github.com/dcc-mcp/dcc-cua/commit/89ff00e48e2eef24459777e9cfb732a9d4d2cd58))
* **host:** stop preparation when task authority ends ([0307fda](https://github.com/dcc-mcp/dcc-cua/commit/0307fdab395fd79c7aed77dcd6afb961fe1192db))
* initialize Windows cursor info size ([13686ee](https://github.com/dcc-mcp/dcc-cua/commit/13686ee78b00ff45439ff6c2ef02476b00558592))
* initialize Windows cursor info size ([1d3c93f](https://github.com/dcc-mcp/dcc-cua/commit/1d3c93f057761f39ffe92caba296713c69636c75))
* **release:** verify single executable install manifests ([56200f2](https://github.com/dcc-mcp/dcc-cua/commit/56200f2d45348421b01517f3c8006b2fcf65ed51))
* restore maximized windows before frame changes ([d2e8665](https://github.com/dcc-mcp/dcc-cua/commit/d2e86654481ab78c2bba2ba0ee16d5c2fba5c426))
* restore maximized windows before frame changes ([b9d0e09](https://github.com/dcc-mcp/dcc-cua/commit/b9d0e09dd21555fb24231ac985c7f080835d9671))
* suppress only owned indicators during pixel input ([ff5b548](https://github.com/dcc-mcp/dcc-cua/commit/ff5b548538c5d4f5ac17a36480c053d257f2a9c5))
* suppress only owned indicators during pixel input ([1c02661](https://github.com/dcc-mcp/dcc-cua/commit/1c02661d3935df0df063d33190274b2f8f35c207))
* **windows:** dismiss exact native popup menus safely ([7c927ad](https://github.com/dcc-mcp/dcc-cua/commit/7c927ad7154442322f8744dddbbba0db0c07ca9d))
* **windows:** dismiss exact native popup menus safely ([1510e1d](https://github.com/dcc-mcp/dcc-cua/commit/1510e1da62f53cd0e4aa521ab4337524c867b550))
* **windows:** harden native menu cancellation readback ([c7097a4](https://github.com/dcc-mcp/dcc-cua/commit/c7097a42f92e7eb8f01c4b7a18fd266c195308e2))

## [1.10.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.10.0...v1.10.1) (2026-10-16)


### Bug Fixes

* **client:** diagnose unsupported URL endpoints before local IPC ([ed0c0d6](https://github.com/dcc-mcp/dcc-cua/commit/ed0c0d62a124f988939ef498af6d6b6e05e5b24b))
* **release:** require packaged background entry in install manifests ([0ad9790](https://github.com/dcc-mcp/dcc-cua/commit/0ad9790c7a10ce2592f14a253ab1bbc8a4415a11))

## [1.10.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.9.4...v1.10.0) (2026-10-13)


### Features

* expose read-only MCP connection diagnostics ([#312](https://github.com/dcc-mcp/dcc-cua/issues/312)) ([5891425](https://github.com/dcc-mcp/dcc-cua/commit/5891425515dff637d1a21ecad3a8bfcd02cf42de))


### Bug Fixes

* **cli:** add console-free MCP entry point ([#316](https://github.com/dcc-mcp/dcc-cua/issues/316)) ([4bde8f0](https://github.com/dcc-mcp/dcc-cua/commit/4bde8f072ba94c9ec7cf5f33d5bd3e81aab9c0a7))
* **cli:** preserve safe host supervisor errors ([#310](https://github.com/dcc-mcp/dcc-cua/issues/310)) ([8926d5c](https://github.com/dcc-mcp/dcc-cua/commit/8926d5c9fdfe041ee8b4f290b3115613e9c9060e))
* preserve semantic snapshots with cached UIA context ([#318](https://github.com/dcc-mcp/dcc-cua/issues/318)) ([50ac30d](https://github.com/dcc-mcp/dcc-cua/commit/50ac30d8b6aa6a8e13c79641400d17b57d485f61))

## [1.9.4](https://github.com/dcc-mcp/dcc-cua/compare/v1.9.3...v1.9.4) (2026-10-04)


### Bug Fixes

* **core:** preserve Windows desktop execution context in diagnostics ([#308](https://github.com/dcc-mcp/dcc-cua/issues/308)) ([b2a5a14](https://github.com/dcc-mcp/dcc-cua/commit/b2a5a147802ece095d7ba9c2ed1b8aad88186c97))

## [1.9.3](https://github.com/dcc-mcp/dcc-cua/compare/v1.9.2...v1.9.3) (2026-10-03)


### Bug Fixes

* expose scoped window restore in MCP tasks ([#306](https://github.com/dcc-mcp/dcc-cua/issues/306)) ([e36b49e](https://github.com/dcc-mcp/dcc-cua/commit/e36b49eafce12ea7dfa475fa6a1b6af4ebdd77a8))

## [1.9.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.9.1...v1.9.2) (2026-09-26)


### Bug Fixes

* clarify isolated browser startup failures ([42ee098](https://github.com/dcc-mcp/dcc-cua/commit/42ee098973dda965df29e3db9305ef587b9267a2))
* narrow browser startup diagnostic wording ([557e0e8](https://github.com/dcc-mcp/dcc-cua/commit/557e0e8f22ce7dc351a186733bb7b6d1fc9e9d51))

## [1.9.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.9.0...v1.9.1) (2026-09-14)


### Bug Fixes

* recover releases with distinct workflow and source commits ([704086e](https://github.com/dcc-mcp/dcc-cua/commit/704086e7f32967209e48a0f7f5d8d85feecbf322))
* retain native latest during component recovery ([189a9ce](https://github.com/dcc-mcp/dcc-cua/commit/189a9ce2f8f0ba5eb626cac3e0d1eabc50047393))

## [1.9.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.8.3...v1.9.0) (2026-09-13)


### Features

* add bounded continuity state and metrics ([8464aaa](https://github.com/dcc-mcp/dcc-cua/commit/8464aaa2f422c2adb4e79e84a3a516edb55ae8f0))
* add continuity chain contract ([a93c7b4](https://github.com/dcc-mcp/dcc-cua/commit/a93c7b4a08b57a116b0ba0104a239f135cbf3322))


### Bug Fixes

* preserve navigation receipts and bounded continuity completion history
* remove native action authorization popups

* align rust layout limit with test suite ([84c52cd](https://github.com/dcc-mcp/dcc-cua/commit/84c52cdabd98771834b5fce4555ad24d062ec414))
* cancel stale native confirmation prompts ([97dfb3a](https://github.com/dcc-mcp/dcc-cua/commit/97dfb3a1034a93b410912a41754d906b64ef619a))
* classify desktop captures as visual ([2f232dd](https://github.com/dcc-mcp/dcc-cua/commit/2f232ddd6771c70af3d91ed9ef1ddf5476006af6))
* move continuity tests to protocol test module ([24a97e7](https://github.com/dcc-mcp/dcc-cua/commit/24a97e710aaa9174e8b7b94340573fe95d432d9e))
* preserve desktop observation metrics ([0b69531](https://github.com/dcc-mcp/dcc-cua/commit/0b69531782d54ffdab26d57a47467338fca782f8))
* require sequential continuity epochs ([9ca6880](https://github.com/dcc-mcp/dcc-cua/commit/9ca6880adff381e120bdc7bdeba423f354517827))
* use rstest for continuity tests ([ed31f82](https://github.com/dcc-mcp/dcc-cua/commit/ed31f827ca579407701b28e6ba90df3a85e57866))
* validate semantic post snapshot mode ([01e6849](https://github.com/dcc-mcp/dcc-cua/commit/01e68492dd95c458bf182be8aad457e36758c6fc))


### Performance Improvements

* cache scoped Windows UI control paths

* expose host request latency metrics ([3990e9b](https://github.com/dcc-mcp/dcc-cua/commit/3990e9b49d688ecfb3415cce129482cd30eb2724))
* support semantic post-action snapshots ([cc01afd](https://github.com/dcc-mcp/dcc-cua/commit/cc01afda55d167e23006f3d48ccff01b0e54ce59))

## [1.8.3](https://github.com/dcc-mcp/dcc-cua/compare/v1.8.2...v1.8.3) (2026-09-10)


### Bug Fixes

* preserve typed observation timeout details ([3793c67](https://github.com/dcc-mcp/dcc-cua/commit/3793c6793ee77999e46fc965270871b738f80545))

## [1.8.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.8.1...v1.8.2) (2026-09-08)


### Bug Fixes

* bound release ZIP resource usage ([#279](https://github.com/dcc-mcp/dcc-cua/issues/279)) ([33aa2eb](https://github.com/dcc-mcp/dcc-cua/commit/33aa2eb244928df017a31f26abba97ceb019e2cf))
* **capture:** suppress idle peer cursor overlays ([e531374](https://github.com/dcc-mcp/dcc-cua/commit/e5313748fa623f7467f9f629bf76e580a6a94756))
* **cli:** align window action tool contracts ([#284](https://github.com/dcc-mcp/dcc-cua/issues/284)) ([843a776](https://github.com/dcc-mcp/dcc-cua/commit/843a776ef66598e0de68a6276bbc2d67305b2509))
* **cli:** diagnose invalid pixels-only options ([b28804b](https://github.com/dcc-mcp/dcc-cua/commit/b28804b106cda8ccec038a8784b63d60060e7ca6))
* reject ambiguous Codex shell URL input ([#285](https://github.com/dcc-mcp/dcc-cua/issues/285)) ([983288d](https://github.com/dcc-mcp/dcc-cua/commit/983288dceb827a72c8e1abca78c3e03291c054c6))
* reject Windows reparse points in release validation ([7015fc3](https://github.com/dcc-mcp/dcc-cua/commit/7015fc33aeabb6b5cc952ec46402de1686ed1d7d))


### Performance Improvements

* avoid cloning cached accessibility trees ([fdda24e](https://github.com/dcc-mcp/dcc-cua/commit/fdda24e38203d0937ed61ce04e2cc98a646475c7))
* reduce idle MCP server threads ([#286](https://github.com/dcc-mcp/dcc-cua/issues/286)) ([41956c8](https://github.com/dcc-mcp/dcc-cua/commit/41956c8f0bf33fd36739656ba64cfa33a297ecde))

## [1.8.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.8.0...v1.8.1) (2026-09-06)


### Bug Fixes

* make release recovery idempotent ([362cd22](https://github.com/dcc-mcp/dcc-cua/commit/362cd22358f617320a077abc6a822a41924363d4))
* update CUA SDK and unify dependency pins ([b25945e](https://github.com/dcc-mcp/dcc-cua/commit/b25945e00a1620cf350bdeb16a00077569beb9b7))


### Performance Improvements

* bound JSONL request lines ([#274](https://github.com/dcc-mcp/dcc-cua/issues/274)) ([5f1a5ff](https://github.com/dcc-mcp/dcc-cua/commit/5f1a5ff8190f4bdf4bcb4346d5d704c3313a9b4b))
* reuse release verification digests ([#275](https://github.com/dcc-mcp/dcc-cua/issues/275)) ([567f5d5](https://github.com/dcc-mcp/dcc-cua/commit/567f5d5d6af64a87f1bf0c5435deeb21ffb26743))

## [1.8.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.7.1...v1.8.0) (2026-08-31)


### Features

* add Unity runtime perception profile ([a7f0fd5](https://github.com/dcc-mcp/dcc-cua/commit/a7f0fd5788bc872c95d89231cd07a1aa78d747c6))

## [1.7.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.7.0...v1.7.1) (2026-08-31)


### Bug Fixes

* support non-interactive updates and release assets ([#262](https://github.com/dcc-mcp/dcc-cua/issues/262)) ([a97130b](https://github.com/dcc-mcp/dcc-cua/commit/a97130b7fd1160aae4d46cbbcaa073606fd6a8ae))

## [1.7.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.6.2...v1.7.0) (2026-08-31)


### Features

* add protected task authorization ([4ec822a](https://github.com/dcc-mcp/dcc-cua/commit/4ec822a6fefda4ddfb14dc706c76581e5403ca17))
* delegate task authorization to agent hosts ([28ad532](https://github.com/dcc-mcp/dcc-cua/commit/28ad532e5e97bee1a5c86416a3cea816be623f22))


### Bug Fixes

* keep task authorization portable ([ec7f69e](https://github.com/dcc-mcp/dcc-cua/commit/ec7f69ebd9cd44bd072698b6a5204e59dc7bcce5))
* remove redundant task authorization gate ([#261](https://github.com/dcc-mcp/dcc-cua/issues/261)) ([013ac8b](https://github.com/dcc-mcp/dcc-cua/commit/013ac8b18546631cb0c44032148aa0f421ab5c28))

## [1.6.2](https://github.com/dcc-mcp/dcc-cua/compare/v1.6.1...v1.6.2) (2026-08-29)


### Bug Fixes

* stabilize packaged skill bytes ([c108724](https://github.com/dcc-mcp/dcc-cua/commit/c108724924c0e61dd8726f250cec6ab16f993c2f))

## [1.6.1](https://github.com/dcc-mcp/dcc-cua/compare/v1.6.0...v1.6.1) (2026-08-29)


### Bug Fixes

* stabilize release documentation bytes ([523ee29](https://github.com/dcc-mcp/dcc-cua/commit/523ee293f8b967f3925997789b1d0d1ca2902c96))

## [1.6.0](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.7...v1.6.0) (2026-08-29)


### Features

* allow external process termination ([2fad923](https://github.com/dcc-mcp/dcc-cua/commit/2fad923144db03d3b136f74b6b51bbb985a1c7d8))
* **cli:** add exact window selectors to scoped commands ([6afac39](https://github.com/dcc-mcp/dcc-cua/commit/6afac39dadb098a8cd473608fd58fdf82b90c816))
* **cli:** add exact-window pixel snapshots ([94635fb](https://github.com/dcc-mcp/dcc-cua/commit/94635fb7ee61734f8aacfabfe27edeeb876a4544))
* verify cross-client task authorization ([e138d0e](https://github.com/dcc-mcp/dcc-cua/commit/e138d0e05b817724336452c69d784930a2e0a6dc))


### Bug Fixes

* bind browser store readiness evidence ([b0c833b](https://github.com/dcc-mcp/dcc-cua/commit/b0c833bef735621b3de3833663a497f7801730c5))
* bind readiness to environment reviewers ([9d730b9](https://github.com/dcc-mcp/dcc-cua/commit/9d730b95e1e9137e9b9eaecdaf14fcdbd705f19f))
* bind store readiness to main ([a8afd3b](https://github.com/dcc-mcp/dcc-cua/commit/a8afd3bf22adef3acb9cbfc18716bd882352016e))
* **browser:** bind public navigation receipts ([808b1f0](https://github.com/dcc-mcp/dcc-cua/commit/808b1f0c8b9e7bdaff289a54d45108d0688fd70b))
* **browser:** preserve navigation failure receipts ([d8dcfaa](https://github.com/dcc-mcp/dcc-cua/commit/d8dcfaaf83a8990dd74dce050ec1744ed4f6e765))
* **browser:** verify foreground tab activation ([1727c5e](https://github.com/dcc-mcp/dcc-cua/commit/1727c5eae87a701746ec76ad6d7737e972ab105e))
* **browser:** verify navigation completion receipts ([ed235be](https://github.com/dcc-mcp/dcc-cua/commit/ed235bec7b40643ae77d51228001074d384df4ae))
* **ci:** freeze complete workflow execution surface ([c1db0a4](https://github.com/dcc-mcp/dcc-cua/commit/c1db0a4a2ab3e814d3db3bc8f45978ed9c4bb305))
* clarify authorization integration ownership ([e9db883](https://github.com/dcc-mcp/dcc-cua/commit/e9db88390b2b1245ef76d7715afe6782a6c31ae1))
* **cli:** emit command errors on stdout ([84958f9](https://github.com/dcc-mcp/dcc-cua/commit/84958f9cd42196e0c022a48123ab4c4e6b8fb381))
* **cli:** enforce structured stream boundaries ([af8a5d6](https://github.com/dcc-mcp/dcc-cua/commit/af8a5d66858a4281b22ed1a51137edb1c24c79cb))
* **client:** contain spawned host diagnostics ([eab0e1e](https://github.com/dcc-mcp/dcc-cua/commit/eab0e1e5c16b654c41255fdbfd2c36df1b3664c9))
* **cli:** gate Windows process helper ([274aca4](https://github.com/dcc-mcp/dcc-cua/commit/274aca4a90a1401f311a830874d5fee494541180))
* **cli:** harden exact window selector validation ([77efd84](https://github.com/dcc-mcp/dcc-cua/commit/77efd84f34e1d04197de1ad8b9166a1ad1720430))
* **cli:** preserve profile watch stream boundaries ([0d78729](https://github.com/dcc-mcp/dcc-cua/commit/0d78729f7bfe556054be99af6bebf47a303aca12))
* **cli:** preserve protocol failure boundaries ([5e5d420](https://github.com/dcc-mcp/dcc-cua/commit/5e5d42054fda56f2cdf078216e8a09a63d1716a1))
* **cli:** publish doctor diagnostics before shutdown ([238403a](https://github.com/dcc-mcp/dcc-cua/commit/238403a852a727735301f6a29c804d474dda2bce))
* **cli:** sanitize public command failures ([8ab88dc](https://github.com/dcc-mcp/dcc-cua/commit/8ab88dcb5f189cc5f0152c4b843930628bbf88bb))
* contain private worker diagnostics ([b6d6681](https://github.com/dcc-mcp/dcc-cua/commit/b6d668199a3c3b0480c614c0be699c777c7aff78))
* **core:** gate pixel capture by platform ([dc65d22](https://github.com/dcc-mcp/dcc-cua/commit/dc65d229ff9f8365dd0d734e167365a923b3d568))
* distinguish missing accessibility providers ([2db0d1e](https://github.com/dcc-mcp/dcc-cua/commit/2db0d1e8d090d5158ca332b275e7ad1f29208ef5))
* enforce owned capture exclusion ([04c39cb](https://github.com/dcc-mcp/dcc-cua/commit/04c39cb78c39cf36434aa9bb0b28ae4c18dabe2d))
* enforce snapshot coordinate space ([af834d2](https://github.com/dcc-mcp/dcc-cua/commit/af834d2feccf98016bdb30a98812d94895e0340f))
* exclude invalid profiles from default listing ([2862fe9](https://github.com/dcc-mcp/dcc-cua/commit/2862fe9e70ca2447502f27a53b91e084c3d5a6ee))
* fail closed browser store readiness ([6630c96](https://github.com/dcc-mcp/dcc-cua/commit/6630c96d99494d9ef747f9444ae9c542de1b8d27))
* harden browser readiness CI contract ([3c9ba11](https://github.com/dcc-mcp/dcc-cua/commit/3c9ba11e95a34e6ef5f66c3203e3aabdbc33b014))
* harden browser readiness failure contracts ([f9c1574](https://github.com/dcc-mcp/dcc-cua/commit/f9c157440e33da78410c42948aeb5a97613723f1))
* harden browser readiness redaction ([dd969c4](https://github.com/dcc-mcp/dcc-cua/commit/dd969c4401ffd9b2126c00be418a11d6b89d59af))
* harden browser store ownership reads ([331a78e](https://github.com/dcc-mcp/dcc-cua/commit/331a78e3822e94bd1b4e85d209032f7e16f36768))
* harden browser store readiness contracts ([004055c](https://github.com/dcc-mcp/dcc-cua/commit/004055c224ec0b7cca32a3657dc29e0399d2e411))
* harden CI artifact identity gates ([988e191](https://github.com/dcc-mcp/dcc-cua/commit/988e1918cc863332880298efe4769b4a52148e0a))
* harden exact-window pixel publication ([cb743d8](https://github.com/dcc-mcp/dcc-cua/commit/cb743d802b65d64d598c8ad5b1cbc6cc950ccf4b))
* preserve cross-platform pixel fallback ([c8d9462](https://github.com/dcc-mcp/dcc-cua/commit/c8d94626c35d8cd9f0d7b4090f85b66e2f81dd6e))
* preserve immutable CI readiness contract ([96d4a9d](https://github.com/dcc-mcp/dcc-cua/commit/96d4a9d9192de087ea672628c7aacd6f1ba8db17))
* preserve snapshots after activation refusal ([fff88a7](https://github.com/dcc-mcp/dcc-cua/commit/fff88a724e61c4b2ebd2853c0edfee4f20c8b4cd))
* report unavailable trusted authorization integration ([a96cfae](https://github.com/dcc-mcp/dcc-cua/commit/a96cfae615139318f497b51a33dac46104c0955c))
* retry foreground clicks before dispatch ([2793758](https://github.com/dcc-mcp/dcc-cua/commit/27937581ededc24e609e768399bb428dd8e223c3))

## [1.5.7](https://github.com/dcc-mcp/dcc-cua/compare/v1.5.6...v1.5.7) (2026-08-26)


### Bug Fixes

* bind release refresh to exact main ([e278aa0](https://github.com/dcc-mcp/dcc-cua/commit/e278aa0f12943334e41ad910bd544ddadb366cab))
* confirm unsafe keyboard chords ([f8e69e4](https://github.com/dcc-mcp/dcc-cua/commit/f8e69e4663b372b1ed1b15748e472a00b1ccc50e))
* enforce keyboard policy before semantic routing ([e3a4df0](https://github.com/dcc-mcp/dcc-cua/commit/e3a4df04c9a3fe548e7df1cf3a78c34e29aa2204))
* grant release dispatch permission ([a53cb95](https://github.com/dcc-mcp/dcc-cua/commit/a53cb95dd4af307e3015de6345ee86bceeae48a6))
* harden release refresh contracts ([63b7ec8](https://github.com/dcc-mcp/dcc-cua/commit/63b7ec802f8c90f3f06074e3f2726bdedad039de))
* honor task grants for keyboard input ([#209](https://github.com/dcc-mcp/dcc-cua/issues/209)) ([d749c5c](https://github.com/dcc-mcp/dcc-cua/commit/d749c5c98617097dda796a9df33fdacbd5fd12e5))
* normalize PowerShell release diagnostics ([f4ea5d3](https://github.com/dcc-mcp/dcc-cua/commit/f4ea5d3597ae78c121ced09a02a50e27c71cebb0))
* reject unsafe release archive paths ([012b616](https://github.com/dcc-mcp/dcc-cua/commit/012b616434f90f0c7be13c41a512e13a520019c7))
* validate extension assets before publish ([e302d69](https://github.com/dcc-mcp/dcc-cua/commit/e302d69267c53218686a19ccdab1a201339fa3a7))
* verify immutable release transport ([ebc5d40](https://github.com/dcc-mcp/dcc-cua/commit/ebc5d40539a512d2d9f82767b3b82202698dd66b))

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
