import contextlib
import hashlib
import json
import re
import unittest
from pathlib import Path

WORKFLOW = Path(__file__).parent.parent / ".github" / "workflows" / "release-please.yml"
CI_WORKFLOW = Path(__file__).parent.parent / ".github" / "workflows" / "ci-checks.yml"
MACOS_TOOLCHAIN_ACTION = (
    Path(__file__).parent.parent
    / ".github"
    / "actions"
    / "select-macos-toolchain"
    / "action.yml"
)
PREFLIGHT_WORKFLOW = (
    Path(__file__).parent.parent
    / ".github"
    / "workflows"
    / "browser-store-preflight.yml"
)
SYNC_SCRIPT = Path(__file__).with_name("sync-cargo-workspace-version.ps1")
REFRESH_SCRIPT = Path(__file__).with_name("refresh-release-please-prs.ps1")
GUI_E2E_SCRIPT = Path(__file__).with_name("run-gui-e2e.ps1")
CLI_E2E_SCRIPT = Path(__file__).with_name("test-cli-e2e.ps1")
ROOT = Path(__file__).parent.parent
GIT_ATTRIBUTES = ROOT / ".gitattributes"
MARKETPLACE = ROOT / ".claude-plugin" / "marketplace.json"
ROOT_PLUGIN = ROOT / ".codex-plugin" / "plugin.json"
MARKETPLACE_PLUGIN = ROOT / "plugins" / "dcc-cua-computer-use"
README = ROOT / "README.md"
README_ZH = ROOT / "README.zh-CN.md"
CHECKOUT_ACTION = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"
DOWNLOAD_ACTION = "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093"
UPLOAD_ACTION = "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
RELEASE_PLEASE_ACTION = (
    "googleapis/release-please-action@45996ed1f6d02564a971a2fa1b5860e934307cf7"
)
CI_EXECUTABLE_SURFACE_SHA256 = (
    "98784174246b989e2914ba95ff49cac3a06ce735ac54932cc0d5c6a45c62ca4e"
)


_YAML_META_TOKEN = re.compile(r"(?:^|[\s\[{:,-])[&*][A-Za-z_][\w.-]*")
_YAML_TAG_TOKEN = re.compile(r"(?:^|[\s\[{:,-])![A-Za-z_][\w!.-]*")


def _strip_yaml_comment(value: str) -> str:
    quote = None
    escaped = False
    for index, character in enumerate(value):
        if quote == '"' and character == "\\" and not escaped:
            escaped = True
            continue
        if character in ("'", '"') and not escaped:
            if quote is None:
                quote = character
            elif quote == character:
                quote = None
        if character == "#" and quote is None and (
            index == 0 or value[index - 1].isspace()
        ):
            return value[:index].rstrip()
        escaped = False
    if quote is not None:
        raise AssertionError("unterminated quoted YAML scalar")
    return value.rstrip()


def _split_yaml_mapping_entry(value: str) -> tuple[str, str]:
    quote = None
    escaped = False
    for index, character in enumerate(value):
        if quote == '"' and character == "\\" and not escaped:
            escaped = True
            continue
        if character in ("'", '"') and not escaped:
            if quote is None:
                quote = character
            elif quote == character:
                quote = None
        elif character == ":" and quote is None:
            key = value[:index].strip()
            if not key:
                raise AssertionError("empty YAML mapping key")
            if key == "<<":
                raise AssertionError("YAML merge keys are forbidden")
            return key, value[index + 1 :].strip()
        escaped = False
    raise AssertionError(f"expected YAML mapping entry: {value!r}")


def _assert_plain_yaml_scalar(value: str) -> str:
    scalar = _strip_yaml_comment(value)
    if not scalar:
        raise AssertionError("empty inline YAML scalar")
    if _YAML_META_TOKEN.search(scalar) or _YAML_TAG_TOKEN.search(scalar):
        raise AssertionError("YAML anchors, aliases, and tags are forbidden")
    return scalar


class _RestrictedWorkflowYamlParser:
    """Parse the reviewed GitHub Actions YAML subset without implicit YAML types."""

    def __init__(self, workflow: str):
        normalized = workflow.replace("\r\n", "\n").replace("\r", "\n")
        if normalized.startswith("\ufeff"):
            raise AssertionError("YAML byte-order marks are forbidden")
        self.lines = normalized.split("\n")
        for line in self.lines:
            if "\t" in line[: len(line) - len(line.lstrip())]:
                raise AssertionError("tabs are forbidden in YAML indentation")
            if line.startswith(("%", "---", "...")):
                raise AssertionError("YAML directives and document markers are forbidden")

    @staticmethod
    def _indent(line: str) -> int:
        return len(line) - len(line.lstrip(" "))

    def _next_content(self, index: int) -> tuple[int, str] | None:
        while index < len(self.lines):
            stripped = self.lines[index].strip()
            if stripped and not stripped.startswith("#"):
                return index, self.lines[index]
            index += 1
        return None

    def parse(self):
        first = self._next_content(0)
        if first is None:
            raise AssertionError("workflow YAML is empty")
        index, line = first
        if self._indent(line) != 0:
            raise AssertionError("workflow root must start at indentation zero")
        value, index = self._parse_node(index, 0)
        if self._next_content(index) is not None:
            raise AssertionError("unexpected trailing YAML content")
        return value

    def _parse_node(self, index: int, indent: int):
        line = self.lines[index]
        if self._indent(line) != indent:
            raise AssertionError("unexpected YAML indentation")
        if line[indent:].startswith("- "):
            return self._parse_sequence(index, indent)
        return self._parse_mapping(index, indent)

    def _parse_mapping(self, index: int, indent: int, initial=None):
        mapping = {} if initial is None else initial
        while True:
            content = self._next_content(index)
            if content is None:
                return mapping, len(self.lines)
            index, line = content
            current_indent = self._indent(line)
            if current_indent < indent:
                return mapping, index
            if current_indent > indent:
                raise AssertionError("unexpected nested YAML mapping content")
            text = line[indent:]
            if text.startswith("- "):
                return mapping, index
            key, raw_value = _split_yaml_mapping_entry(text)
            if key in mapping:
                raise AssertionError(f"duplicate YAML mapping key: {key}")
            mapping[key], index = self._parse_mapping_value(
                raw_value, index + 1, indent
            )

    def _parse_mapping_value(self, raw_value: str, index: int, key_indent: int):
        raw_value = _strip_yaml_comment(raw_value)
        if raw_value in ("|", "|-", "|+", ">", ">-", ">+"):
            return self._parse_block_scalar(index, key_indent, raw_value)
        if raw_value:
            return _assert_plain_yaml_scalar(raw_value), index
        child = self._next_content(index)
        if child is None:
            return None, len(self.lines)
        child_index, child_line = child
        child_indent = self._indent(child_line)
        if child_indent <= key_indent:
            return None, child_index
        if child_indent != key_indent + 2:
            raise AssertionError("YAML nesting must use two-space indentation")
        return self._parse_node(child_index, child_indent)

    def _parse_block_scalar(self, index: int, key_indent: int, style: str):
        block_indent = key_indent + 2
        block_lines = []
        while index < len(self.lines):
            line = self.lines[index]
            if line.strip() and self._indent(line) <= key_indent:
                break
            if line.strip() and self._indent(line) < block_indent:
                raise AssertionError("invalid block scalar indentation")
            block_lines.append(line[block_indent:] if line.strip() else "")
            index += 1
        while block_lines and not block_lines[-1]:
            block_lines.pop()
        text = "\n".join(block_lines)
        if style in ("|", ">"):
            text += "\n"
        return {"style": style, "text": text}, index

    def _parse_sequence(self, index: int, indent: int):
        sequence = []
        while True:
            content = self._next_content(index)
            if content is None:
                return sequence, len(self.lines)
            index, line = content
            current_indent = self._indent(line)
            if current_indent < indent:
                return sequence, index
            if current_indent != indent or not line[indent:].startswith("- "):
                return sequence, index
            item = line[indent + 2 :]
            try:
                key, raw_value = _split_yaml_mapping_entry(item)
            except AssertionError:
                sequence.append(_assert_plain_yaml_scalar(item))
                index += 1
                continue
            mapping = {}
            mapping[key], index = self._parse_mapping_value(
                raw_value, index + 1, indent + 2
            )
            mapping, index = self._parse_mapping(index, indent + 2, mapping)
            sequence.append(mapping)


def _ci_executable_surface(workflow: str) -> str:
    parsed = _RestrictedWorkflowYamlParser(workflow).parse()
    expected_root_keys = {"name", "on", "permissions", "concurrency", "jobs"}
    if set(parsed) != expected_root_keys:
        raise AssertionError("unexpected top-level CI workflow keys")
    jobs = parsed["jobs"]
    if not isinstance(jobs, dict) or set(jobs) != {
        "browser-extension",
        "policy",
        "verify",
        "e2e",
    }:
        raise AssertionError("CI workflow job graph differs from the reviewed graph")
    return json.dumps(parsed, ensure_ascii=True, sort_keys=True, separators=(",", ":"))


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_cli_failure_stdout_contract_cannot_be_orphaned_from_ci(self):
        workflow = CI_WORKFLOW.read_text(encoding="utf-8")
        script = CLI_E2E_SCRIPT.read_text(encoding="utf-8")
        e2e = workflow[workflow.index("  e2e:") :]

        self.assertIn("cargo build --release --locked -p dcc-cua-cli", e2e)
        self.assertIn("./scripts/test-cli-e2e.ps1 -Binary $binary", e2e)
        self.assertIn("Assert-CommandFailureStdoutContract", script)
        self.assertIn("RedirectStandardOutput", script)
        self.assertIn("RedirectStandardError", script)
        self.assertIn("definitely-not-a-command", script)
        self.assertIn("RELEASE_PRIVATE_ARGUMENT_8e1ab4", script)
        self.assertIn("RELEASE_PRIVATE_OPTION_351cc7", script)
        self.assertIn("dcc-cua could not complete the command", script)
        self.assertIn("fixtures\\hostile_host.rs", script)
        self.assertIn("CHILD_PRIVATE_DIAGNOSTIC_7e87d1", script)
        self.assertIn('"host-call", "--spawn", $hostileHost', script)
        self.assertIn("os.pipe()", script)
        self.assertIn("os.close(read_fd)", script)
        self.assertIn("process.communicate(timeout=10)", script)
        self.assertIn("process.kill()", script)
        self.assertIn(
            "closed stdout did not emit exactly one fixed safe diagnostic", script
        )

    def test_final_native_archives_execute_documentation_parity_gate(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        for document in ("README.md", "README.zh-CN.md"):
            self.assertIn(f"cp {document} target/release/{document}", workflow)
            self.assertIn(document, workflow)
        self.assertGreaterEqual(
            workflow.count("release_integrity.py verify-native-docs"), 2
        )
        self.assertIn('--archive "$archive" --source .', workflow)
        self.assertIn('--archive "$native_archive" --source .', workflow)

    def test_release_documentation_has_platform_stable_line_endings(self):
        attributes = {
            line.strip()
            for line in GIT_ATTRIBUTES.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        }

        self.assertTrue(
            {
                "/README.md text eol=lf",
                "/README.zh-CN.md text eol=lf",
            }.issubset(attributes)
        )
        self.assertNotIn(b"\r", README.read_bytes())
        self.assertNotIn(b"\r", README_ZH.read_bytes())

    def test_release_matrix_builds_every_supported_native_target(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        expected_jobs = (
            ("windows-latest", "windows-x86_64", "x86_64-pc-windows-msvc"),
            ("ubuntu-latest", "linux-x86_64", "x86_64-unknown-linux-gnu"),
            ("macos-26", "macos-aarch64", "aarch64-apple-darwin"),
            ("macos-26-intel", "macos-x86_64", "x86_64-apple-darwin"),
        )
        build_matrix = workflow[
            workflow.index("  build:") : workflow.index("  attach-assets:")
        ]
        for runner, artifact, target in expected_jobs:
            self.assertIn(
                f"- os: {runner}\n            platform: {artifact}\n"
                f"            target: {target}\n",
                build_matrix,
            )
        self.assertIn('if [ "$host" != "$EXPECTED_TARGET" ]; then', build_matrix)
        self.assertIn("EXPECTED_TARGET: ${{ matrix.target }}", build_matrix)

    def test_release_upload_fails_closed_until_all_target_assets_exist(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        attach_assets = workflow[
            workflow.index("  attach-assets:") : workflow.index(
                "  package-browser-extension:"
            )
        ]

        checkout = attach_assets.index(CHECKOUT_ACTION)
        download = attach_assets.index(DOWNLOAD_ACTION)
        verify = attach_assets.index("scripts/release_integrity.py verify-provenance")
        upload = attach_assets.index("gh release upload")
        self.assertLess(checkout, download)
        self.assertLess(download, verify)
        self.assertLess(verify, upload)
        self.assertIn(
            "ref: ${{ needs.release-please.outputs.source_sha }}", attach_assets
        )

    def test_native_release_download_uses_the_exact_verified_matrix_plan(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        build_matrix = workflow[
            workflow.index("  build:") : workflow.index("  attach-assets:")
        ]
        consolidate_native = workflow[
            workflow.index("  consolidate-native:") : workflow.index("  attach-assets:")
        ]

        self.assertIn("name: dcc-cua-native-${{ matrix.platform }}", build_matrix)
        self.assertIn(
            "scripts/release_integrity.py write-native-plan", consolidate_native
        )
        self.assertIn("actions/artifacts/$artifact_id/zip", consolidate_native)
        self.assertIn("scripts/release_integrity.py verify-extract", consolidate_native)
        self.assertNotIn("pattern: dcc-cua-native-*", consolidate_native)

    def test_intel_macos_release_target_runs_the_native_ci_contract(self):
        workflow = CI_WORKFLOW.read_text(encoding="utf-8")

        self.assertEqual(workflow.count("macos-26-intel"), 2)
        self.assertGreaterEqual(
            workflow.count("if: startsWith(matrix.os, 'macos-')"), 3
        )
        self.assertNotIn("if: matrix.os == 'macos-latest'", workflow)

    def test_macos_gui_fixture_is_rebuilt_for_the_exact_runner_architecture(self):
        script = GUI_E2E_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("$macArchitecture = (& uname -m).Trim()", script)
        self.assertIn('$macTarget = "$macArchitecture-apple-macos13.0"', script)
        self.assertIn("& xcrun swiftc", script)
        self.assertIn("-target $macTarget", script)
        self.assertIn("& file $appKitExecutable", script)
        self.assertIn("does not match runner architecture", script)

    def test_macos_builds_share_a_pinned_supported_xcode_and_sdk_contract(self):
        release = WORKFLOW.read_text(encoding="utf-8")
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        action = MACOS_TOOLCHAIN_ACTION.read_text(encoding="utf-8")

        for workflow in (release, ci):
            self.assertIn("macos-26-intel", workflow)
            self.assertNotIn("macos-15-intel", workflow)
            self.assertIn("./.github/actions/select-macos-toolchain", workflow)
        self.assertEqual(release.count("./.github/actions/select-macos-toolchain"), 1)
        self.assertEqual(ci.count("./.github/actions/select-macos-toolchain"), 2)
        release_build = release[
            release.index("  build:") : release.index("  attach-assets:")
        ]
        self.assertLess(
            release_build.index(CHECKOUT_ACTION),
            release_build.index("./.github/actions/select-macos-toolchain"),
        )
        self.assertLess(
            release_build.index("./.github/actions/select-macos-toolchain"),
            release_build.index("cargo build --release --locked"),
        )
        self.assertIn("/Applications/Xcode_26.6.app/Contents/Developer", action)
        self.assertIn('actual_arch="$(uname -m)"', action)
        self.assertIn('if [ "$actual_arch" != "$EXPECTED_ARCH" ]; then', action)
        self.assertIn('xcode_version="$(xcodebuild -version', action)
        self.assertIn('if [ "$xcode_version" != "26.6" ]; then', action)
        self.assertIn('sdk_version="$(xcrun --sdk macosx --show-sdk-version)"', action)
        self.assertIn('if [[ "$sdk_version" != 26.* ]]; then', action)

    def test_policy_ci_runs_the_release_asset_verifier_regressions(self):
        workflow = CI_WORKFLOW.read_text(encoding="utf-8")

        self.assertIn(
            "python -B -m unittest scripts.test_verify_release_assets", workflow
        )
        self.assertIn("python -B -m unittest scripts.test_release_integrity", workflow)

    def test_release_refresh_regressions_run_in_policy_and_release_validation(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        release = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn(
            "python -B -m unittest scripts.test_refresh_release_please_prs", ci
        )
        self.assertIn("scripts.test_refresh_release_please_prs", release)

    def test_ci_source_sha_event_regressions_run_in_policy_and_release_validation(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        release = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("python -B -m unittest scripts.test_select_ci_source_sha", ci)
        self.assertIn("scripts.test_select_ci_source_sha", release)

    def assert_ci_source_checkout_contract(self, workflow, *, subtests=True):
        surface_digest = hashlib.sha256(
            _ci_executable_surface(workflow).encode("utf-8")
        ).hexdigest()
        self.assertEqual(surface_digest, CI_EXECUTABLE_SURFACE_SHA256)
        jobs = (
            ("  browser-extension:", "  policy:"),
            ("  policy:", "  verify:"),
            ("  verify:", "  e2e:"),
            ("  e2e:", None),
        )
        checkout_marker = f"      - uses: {CHECKOUT_ACTION}"
        select_marker = "      - name: Select immutable CI source SHA"
        verify_marker = "      - name: Verify exact CI source checkout"
        for start_marker, end_marker in jobs:
            start = workflow.index(start_marker)
            end = workflow.index(end_marker, start) if end_marker else len(workflow)
            job = "\n".join(
                line
                for line in workflow[start:end].splitlines()
                if not line.lstrip().startswith("#")
            )
            context = (
                self.subTest(job=start_marker.strip())
                if subtests
                else contextlib.nullcontext()
            )
            with context:
                self.assertEqual(job.count(select_marker), 1)
                self.assertEqual(job.count(checkout_marker), 1)
                self.assertEqual(job.count("actions/checkout@"), 1)
                self.assertEqual(job.count(verify_marker), 1)
                select = job.index(select_marker)
                checkout = job.index(checkout_marker)
                verify = job.index(verify_marker)
                self.assertLess(select, checkout)
                self.assertLess(checkout, verify)
                next_step = job.index("\n      - ", checkout + 1) + 1
                self.assertEqual(next_step, verify)

                select_step = job[select:checkout]
                for variable, expression in (
                    ("CI_EVENT_NAME", "${{ github.event_name }}"),
                    ("CI_EVENT_SHA", "${{ github.sha }}"),
                    (
                        "CI_PULL_REQUEST_HEAD_SHA",
                        "${{ github.event.pull_request.head.sha }}",
                    ),
                ):
                    self.assertIn(f"{variable}: {expression}", select_step)
                self.assertIn(
                    'pull_request) source_sha="$CI_PULL_REQUEST_HEAD_SHA"', select_step
                )
                self.assertIn(
                    'push|workflow_dispatch) source_sha="$CI_EVENT_SHA"', select_step
                )
                self.assertIn("*[!0-9a-f]*|'')", select_step)
                self.assertIn('[ "${#source_sha}" -ne 40 ]', select_step)
                self.assertIn(
                    'printf \'sha=%s\\n\' "$source_sha" >> "$GITHUB_OUTPUT"',
                    select_step,
                )

                checkout_end = job.index("\n      - ", checkout + 1)
                checkout_step = job[checkout:checkout_end]
                self.assertIn("ref: ${{ steps.ci-source.outputs.sha }}", checkout_step)
                self.assertIn("persist-credentials: false", checkout_step)
                self.assertNotIn("repository:", checkout_step)
                self.assertNotIn("path:", checkout_step)
                self.assertNotIn("ref: ${{ github.sha }}", checkout_step)

                verify_end = job.find("\n      - ", verify + 1)
                verify_step = job[verify:] if verify_end < 0 else job[verify:verify_end]
                self.assertIn(
                    "EXPECTED_SOURCE_SHA: ${{ steps.ci-source.outputs.sha }}",
                    verify_step,
                )
                self.assertIn(
                    "actual_source_sha=\"$(git rev-parse --verify 'HEAD^{commit}')\"",
                    verify_step,
                )
                self.assertIn(
                    'if [ "$actual_source_sha" != "$EXPECTED_SOURCE_SHA" ]; then',
                    verify_step,
                )
                trusted_integrity = (
                    'git show "$EXPECTED_SOURCE_SHA:'
                    'scripts/verify_ci_source_integrity.py" |\n'
                    "            python -B - --repository . "
                    '--expected "$EXPECTED_SOURCE_SHA"'
                )
                self.assertIn(trusted_integrity, verify_step)
                self.assertGreaterEqual(job.count(trusted_integrity), 2)

    def test_ci_builds_only_the_event_selected_immutable_source_checkout(self):
        self.assert_ci_source_checkout_contract(CI_WORKFLOW.read_text(encoding="utf-8"))

    def test_ci_source_checkout_rejects_an_additional_executable_job(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            "jobs:\n  browser-extension:",
            "jobs:\n"
            "  unreviewed-execution:\n"
            "    runs-on: ubuntu-latest\n"
            "    steps:\n"
            "      - run: printf 'unreviewed execution\\n'\n\n"
            "  browser-extension:",
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_job_defaults_that_mutate_before_build(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            "  verify:\n    timeout-minutes: 45",
            "  verify:\n"
            "    defaults:\n"
            "      run:\n"
            "        shell: bash -c 'grep -q \"cargo check\" {0} && "
            "git restore --source attacker-ref --worktree -- .; bash {0}'\n"
            "    timeout-minutes: 45",
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_execution_affecting_job_mappings(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        insertions = (
            "    env:\n      UNREVIEWED_EXECUTION: enabled\n",
            "    permissions:\n      contents: write\n",
            "    container: ubuntu:24.04\n",
            "    services:\n      helper:\n        image: redis:7\n",
        )
        for insertion in insertions:
            mutated = ci.replace(
                "  verify:\n    timeout-minutes: 45",
                f"  verify:\n{insertion}    timeout-minutes: 45",
                1,
            )
            with self.subTest(insertion=insertion), self.assertRaises(AssertionError):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_execution_affecting_step_mappings(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        original = "      - run: cargo check --workspace --all-targets --locked"
        mutations = (
            original + "\n        name: Unreviewed name",
            original + "\n        id: unreviewed-id",
            original + "\n        env:\n          UNREVIEWED_EXECUTION: enabled",
            original + "\n        if: always()",
            original + "\n        shell: bash",
            original + "\n        working-directory: crates",
            original + "\n        timeout-minutes: 44",
            original + "\n        continue-on-error: true",
        )
        for replacement in mutations:
            mutated = ci.replace(original, replacement, 1)
            with (
                self.subTest(replacement=replacement),
                self.assertRaises(AssertionError),
            ):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_pre_and_post_reverify_mutations(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        reverify = "      - name: Reverify immutable source before build"
        cargo_check = "      - run: cargo check --workspace --all-targets --locked"
        verify_start = ci.index("  verify:")
        reverify_start = ci.index(reverify, verify_start)
        before_reverify = (
            ci[:reverify_start]
            + "      - run: git restore --source attacker-ref --worktree -- .\n"
            + ci[reverify_start:]
        )
        mutations = (
            before_reverify,
            ci.replace(
                cargo_check,
                "      - run: git restore --source attacker-ref --worktree -- .\n"
                + cargo_check,
                1,
            ),
        )
        for mutated in mutations:
            with self.subTest(), self.assertRaises(AssertionError):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_commented_and_multiline_decoys(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        original = "      - run: cargo check --workspace --all-targets --locked"
        mutations = (
            ci.replace(
                original,
                "      # - run: cargo check --workspace --all-targets --locked\n"
                "      - run: printf 'decoy bypass\\n'",
                1,
            ),
            ci.replace(
                original,
                "      - run: |\n"
                "          GIT=git\n"
                "          source_swap() { \"$@\"; }\n"
                "          source_swap \"$GIT\" restore --source attacker-ref "
                "--worktree -- .\n"
                "          cargo check --workspace --all-targets --locked",
                1,
            ),
        )
        for mutated in mutations:
            with self.subTest(), self.assertRaises(AssertionError):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_workflow_parser_rejects_anchors_aliases_and_duplicate_keys(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutations = (
            ci.replace(
                "    timeout-minutes: 45",
                "    env: &unreviewed-env\n"
                "      UNREVIEWED_EXECUTION: enabled\n"
                "    timeout-minutes: 45",
                1,
            ),
            ci.replace(
                "      - run: cargo check --workspace --all-targets --locked",
                "      - run: cargo check --workspace --all-targets --locked\n"
                "        env: *unreviewed-env",
                1,
            ),
            ci.replace(
                "      - run: cargo check --workspace --all-targets --locked",
                "      - run: cargo check --workspace --all-targets --locked\n"
                "        run: printf 'duplicate run\\n'",
                1,
            ),
        )
        for mutated in mutations:
            with self.subTest(), self.assertRaises(AssertionError):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_reviewer_merge_ref_counterexample(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            "ref: ${{ steps.ci-source.outputs.sha }}",
            "ref: ${{ github.sha }}",
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_missing_ref_and_commented_decoys(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        for replacement in (
            "# ref: ${{ steps.ci-source.outputs.sha }}",
            "fetch-depth: 1",
        ):
            mutated = ci.replace(
                "ref: ${{ steps.ci-source.outputs.sha }}", replacement, 1
            )
            with (
                self.subTest(replacement=replacement),
                self.assertRaises(AssertionError),
            ):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_later_checkout_and_secondary_repository(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutations = (
            ci.replace(
                "      - run: npm --prefix browser-extension/chrome ci",
                "      - uses: actions/checkout@v7\n"
                "      - run: npm --prefix browser-extension/chrome ci",
                1,
            ),
            ci.replace(
                "ref: ${{ steps.ci-source.outputs.sha }}",
                "repository: attacker/decoy\n          ref: ${{ steps.ci-source.outputs.sha }}",
                1,
            ),
        )
        for mutated in mutations:
            with self.subTest(), self.assertRaises(AssertionError):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_raw_post_verification_git_replacements(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        insertion = "      - uses: dtolnay/rust-toolchain@stable"
        for command in (
            "git read-tree attacker-ref",
            "git restore --source attacker-ref --worktree -- .",
            "git archive attacker-ref | tar -x -f -",
        ):
            mutated = ci.replace(
                insertion,
                f"      - run: {command}\n{insertion}",
                1,
            )
            with (
                self.subTest(command=command),
                self.assertRaises(AssertionError),
            ):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_checkout_rejects_indirect_git_and_extra_step_surface(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        insertion = "      - uses: dtolnay/rust-toolchain@stable"
        commands = (
            'GIT=git; "$GIT" restore --source attacker-ref --worktree -- .',
            "alias source_swap=git; source_swap checkout attacker-ref -- .",
            'source_swap() { "$@"; }; source_swap git read-tree attacker-ref',
            "GIT='g'\\\n          'it'; \"$GIT\" restore --source attacker-ref --worktree -- .",
            "printf 'executable decoy\\n'",
        )
        for command in commands:
            mutated = ci.replace(
                insertion,
                f"      - run: {command}\n{insertion}",
                1,
            )
            with (
                self.subTest(command=command),
                self.assertRaises(AssertionError),
            ):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_ci_source_integrity_rejects_mutable_or_commented_verifier_decoys(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        trusted = (
            'git show "$EXPECTED_SOURCE_SHA:scripts/verify_ci_source_integrity.py" |\n'
            '            python -B - --repository . --expected "$EXPECTED_SOURCE_SHA"'
        )
        for replacement in (
            (
                "python -B scripts/verify_ci_source_integrity.py "
                '--repository . --expected "$EXPECTED_SOURCE_SHA"'
            ),
            "# " + trusted,
        ):
            mutated = ci.replace(trusted, replacement, 1)
            with (
                self.subTest(replacement=replacement),
                self.assertRaises(AssertionError),
            ):
                self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_public_docs_list_every_native_release_target(self):
        english = README.read_text(encoding="utf-8")
        chinese = README_ZH.read_text(encoding="utf-8")
        for target in (
            "x86_64-pc-windows-msvc",
            "x86_64-unknown-linux-gnu",
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
        ):
            self.assertIn(target, english)
            self.assertIn(target, chinese)
        self.assertIn("macos-26-intel", english)
        self.assertIn("macos-26-intel", chinese)
        self.assertNotIn("current signed `dcc-cua` binary", english)
        self.assertIn("not currently platform-signed", english)
        self.assertIn("当前原生可执行文件没有平台代码签名", chinese)
        self.assertIn("raw workflow artifact ZIP", english)
        self.assertIn("发布后回读", chinese)
        self.assertIn("real-user raw-input", english)
        self.assertIn("raw-input", chinese)

    def test_release_archive_includes_the_plugin_and_mcp_bridge(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("cp -R .claude-plugin target/release/.claude-plugin", workflow)
        self.assertIn("cp -R .codex-plugin target/release/.codex-plugin", workflow)
        self.assertIn("cp .mcp.json target/release/.mcp.json", workflow)
        self.assertIn("cp -R plugins target/release/plugins", workflow)
        self.assertIn(
            "assets skills plugins .claude-plugin .codex-plugin .mcp.json", workflow
        )

    def test_native_archives_are_smoked_only_after_uploaded_byte_readback(self):
        self.test_release_native_archives_are_verified_from_exact_uploaded_bytes()

    def assert_pr_ci_final_archive_contract(self, ci):
        self.assert_ci_source_checkout_contract(ci)
        self.assert_native_uploaded_readback_contract(
            ci,
            "  e2e:",
            None,
            "Package final native archive",
            "upload-pr-native",
            None,
        )

    def test_pr_ci_packages_and_smokes_every_final_native_archive(self):
        self.assert_pr_ci_final_archive_contract(
            CI_WORKFLOW.read_text(encoding="utf-8")
        )

    def test_pr_ci_final_archive_contract_rejects_a_commented_verifier(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            "          python -B scripts/verify_uploaded_artifact.py \\",
            "          # python -B scripts/verify_uploaded_artifact.py \\",
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_pr_ci_final_archive_contract(mutated)

    def test_pr_ci_final_archive_contract_rejects_local_rehash_decoy(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            "          python -B scripts/verify_uploaded_artifact.py \\",
            "          sha256sum dist/*",
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_pr_ci_final_archive_contract(mutated)

    def test_pr_ci_final_archive_contract_rejects_mutable_copy_decoy(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            '            --bundle "$ARTIFACT_BUNDLE" \\',
            '            --bundle "dist/local-copy.zip" \\',
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_pr_ci_final_archive_contract(mutated)

    def test_pr_ci_final_archive_contract_rejects_unrelated_artifact_id(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            "ARTIFACT_ID: ${{ steps.upload-pr-native.outputs.artifact-id }}",
            "ARTIFACT_ID: 123",
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_pr_ci_final_archive_contract(mutated)

    def test_pr_ci_final_archive_contract_rejects_merge_ref_head(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            "ref: ${{ steps.ci-source.outputs.sha }}",
            "ref: ${{ github.sha }}",
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_ci_source_checkout_contract(mutated, subtests=False)

    def test_pr_ci_final_archive_contract_rejects_name_lookup(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        mutated = ci.replace(
            'actions/artifacts/${ARTIFACT_ID}/zip"',
            'actions/artifacts?name=dcc-cua-pr-native/zip"',
            1,
        )
        with self.assertRaises(AssertionError):
            self.assert_pr_ci_final_archive_contract(mutated)

    def test_pr_ci_final_archive_contract_rejects_post_verifier_pre_upload_mutation(
        self,
    ):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        upload_marker = f"      - uses: {UPLOAD_ACTION}"
        upload_start = ci.index(upload_marker, ci.index("  e2e:"))
        upload_end = ci.index("\n      - ", upload_start + 1) + 1
        upload_block = ci[upload_start:upload_end]
        without_upload = ci[:upload_start] + ci[upload_end:]
        mutation = (
            "\n      - name: Mutate after verifier before upload\n"
            "        shell: bash\n"
            "        run: printf mutation >> dist/unchecked-archive\n"
        )
        mutated = without_upload + mutation + upload_block
        with self.assertRaises(AssertionError):
            self.assert_pr_ci_final_archive_contract(mutated)

    def assert_native_uploaded_readback_contract(
        self,
        workflow,
        job_start,
        job_end,
        package_name,
        upload_id,
        expected_head_sha,
    ):
        start = workflow.index(job_start)
        end = workflow.index(job_end, start) if job_end else len(workflow)
        job = workflow[start:end]
        executable = "\n".join(
            line for line in job.splitlines() if not line.lstrip().startswith("#")
        )

        package_marker = f"      - name: {package_name}"
        upload_marker = f"      - uses: {UPLOAD_ACTION}"
        download_marker = "      - name: Download exact native artifact by ID"
        verify_marker = "      - name: Verify downloaded immutable native artifact"
        for marker in (package_marker, upload_marker, download_marker, verify_marker):
            self.assertEqual(executable.count(marker), 1)
        package = executable.index(package_marker)
        upload = executable.index(upload_marker)
        download = executable.index(download_marker)
        verify = executable.index(verify_marker)
        self.assertLess(package, upload)
        self.assertLess(upload, download)
        self.assertLess(download, verify)
        self.assertNotIn("verify_final_archive.py", executable[package:upload])
        self.assertIn(
            "python scripts/write-install-manifest.py", executable[package:upload]
        )
        self.assertIn("--source-root target/release", executable[package:upload])

        upload_step = executable[upload:download]
        self.assertIn(f"id: {upload_id}", upload_step)
        self.assertIn("path: dist/*", upload_step)
        self.assertIn("if-no-files-found: error", upload_step)
        self.assertIn("overwrite: false", upload_step)

        artifact_id = f"${{{{ steps.{upload_id}.outputs.artifact-id }}}}"
        artifact_digest = f"${{{{ steps.{upload_id}.outputs.artifact-digest }}}}"
        download_step = executable[download:verify]
        self.assertIn(f"ARTIFACT_ID: {artifact_id}", download_step)
        self.assertIn(f"ARTIFACT_DIGEST: {artifact_digest}", download_step)
        self.assertIn('case "$ARTIFACT_ID" in', download_step)
        self.assertIn('mkdir "$artifact_download_root"', download_step)
        self.assertNotIn('mkdir -p "$artifact_download_root"', download_step)
        self.assertIn(
            'artifact_bundle="$artifact_download_root/artifact.zip"', download_step
        )
        self.assertIn(
            'gh api "repos/${GITHUB_REPOSITORY}/actions/artifacts/${ARTIFACT_ID}"',
            download_step,
        )
        self.assertIn(
            'gh api "repos/${GITHUB_REPOSITORY}" > "$artifact_repository_metadata"',
            download_step,
        )
        self.assertIn(
            'gh api "repos/${GITHUB_REPOSITORY}/actions/artifacts/${ARTIFACT_ID}/zip"',
            download_step,
        )
        self.assertNotIn("actions/download-artifact", download_step)
        self.assertNotIn("?name=", download_step)

        verify_step = executable[verify:]
        self.assertIn(f"ARTIFACT_ID: {artifact_id}", verify_step)
        self.assertIn(f"ARTIFACT_DIGEST: {artifact_digest}", verify_step)
        if expected_head_sha:
            self.assertIn(f"EXPECTED_HEAD_SHA: {expected_head_sha}", verify_step)
            expected_head_repository = "${{ github.repository_id }}"
        else:
            self.assertIn(
                "EXPECTED_SOURCE_SHA: ${{ steps.ci-source.outputs.sha }}",
                verify_step,
            )
            expected_head_repository = (
                "${{ github.event.pull_request.head.repo.id || github.repository_id }}"
            )
        self.assertIn(
            "EXPECTED_REPOSITORY_ID: ${{ github.repository_id }}", verify_step
        )
        self.assertIn(
            f"EXPECTED_HEAD_REPOSITORY_ID: {expected_head_repository}", verify_step
        )
        self.assertIn("python -B scripts/verify_uploaded_artifact.py \\", verify_step)
        for argument in (
            '--metadata "$ARTIFACT_METADATA"',
            '--repository-metadata "$ARTIFACT_REPOSITORY_METADATA"',
            '--bundle "$ARTIFACT_BUNDLE"',
            '--artifact-id "$ARTIFACT_ID"',
            '--artifact-digest "$ARTIFACT_DIGEST"',
            '--run-id "$GITHUB_RUN_ID"',
            '--repository-id "$EXPECTED_REPOSITORY_ID"',
            '--head-repository-id "$EXPECTED_HEAD_REPOSITORY_ID"',
            '--head-sha "$EXPECTED_SOURCE_SHA"'
            if expected_head_sha is None
            else '--head-sha "$EXPECTED_HEAD_SHA"',
            "--source-root target/release",
            "--archive-name ",
            "--manifest-name ",
            '--target "$host"',
            '--version "$version"',
            "--extract-root ",
            "--install-root ",
        ):
            self.assertIn(argument, verify_step)

    def test_release_native_archives_are_verified_from_exact_uploaded_bytes(self):
        self.assert_native_uploaded_readback_contract(
            WORKFLOW.read_text(encoding="utf-8"),
            "  build:",
            "  consolidate-native:",
            "Package dcc-cua",
            "upload-native",
            "${{ needs.release-please.outputs.source_sha }}",
        )

    def test_pr_ci_native_archives_are_verified_from_exact_uploaded_bytes(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")
        self.assert_native_uploaded_readback_contract(
            ci,
            "  e2e:",
            None,
            "Package final native archive",
            "upload-pr-native",
            None,
        )

    def test_marketplace_uses_a_bounded_installable_plugin_directory(self):
        marketplace = json.loads(MARKETPLACE.read_text(encoding="utf-8"))
        self.assertEqual(marketplace["name"], "dcc-cua")
        self.assertEqual(marketplace["interface"]["displayName"], "DCC-CUA")
        self.assertEqual(len(marketplace["plugins"]), 1)
        entry = marketplace["plugins"][0]
        self.assertEqual(entry["name"], "dcc-cua-computer-use")
        self.assertEqual(
            entry["source"],
            {
                "source": "local",
                "path": "./plugins/dcc-cua-computer-use",
            },
        )
        self.assertEqual(entry["policy"]["installation"], "AVAILABLE")
        self.assertEqual(entry["policy"]["authentication"], "ON_INSTALL")
        self.assertEqual(entry["category"], "Productivity")
        self.assertNotEqual(entry["source"], ".")

    def test_marketplace_plugin_is_slim_and_matches_the_root_bridge(self):
        manifest_path = MARKETPLACE_PLUGIN / ".codex-plugin" / "plugin.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        root_manifest = json.loads(ROOT_PLUGIN.read_text(encoding="utf-8"))
        self.assertEqual(MARKETPLACE_PLUGIN.name, manifest["name"])
        self.assertEqual(manifest["version"], root_manifest["version"])
        self.assertEqual(manifest["mcpServers"], "./.mcp.json")
        self.assertNotIn("skills", manifest)
        self.assertEqual(
            json.loads((MARKETPLACE_PLUGIN / ".mcp.json").read_text(encoding="utf-8")),
            json.loads((ROOT / ".mcp.json").read_text(encoding="utf-8")),
        )
        self.assertEqual(
            {path.name for path in MARKETPLACE_PLUGIN.iterdir()},
            {".codex-plugin", ".mcp.json"},
        )

    def test_browser_store_preflight_is_short_lived_and_non_mutating(self):
        workflow = PREFLIGHT_WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotIn("push:", workflow)
        self.assertIn("environment: browser-stores", workflow)
        self.assertIn("id-token: write", workflow)
        self.assertIn("token_format: access_token", workflow)
        self.assertIn("access_token_lifetime: 300s", workflow)
        self.assertIn("create_credentials_file: false", workflow)
        self.assertNotIn("publish_browser_extension.py", workflow)
        self.assertNotIn(":upload", workflow)
        self.assertNotIn(":publish", workflow)

    def test_browser_store_jobs_require_the_protected_user_authorization_environment(
        self,
    ):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        for job in (
            "publish-chrome-web-store:",
            "publish-edge-addons:",
            "publish-firefox-addons:",
        ):
            start = workflow.index(job)
            section = workflow[start : start + 2200]
            self.assertIn("environment: browser-stores", section)
            self.assertIn("DCC_CUA_BROWSER_STORE_PUBLISH_READY == 'true'", section)
        self.assertIn("id-token: write", workflow)
        self.assertIn(
            "google-github-actions/auth@7c6bc770dae815cd3e89ee6cdf493a5fab2cc093",
            workflow,
        )
        self.assertIn(
            "FIREFOX_JWT_ISSUER: ${{ secrets.FIREFOX_AMO_API_KEY }}", workflow
        )
        self.assertIn(
            "FIREFOX_JWT_SECRET: ${{ secrets.FIREFOX_AMO_API_SECRET }}", workflow
        )
        self.assertIn("FIREFOX_SOURCES_ZIP:", workflow)
        self.assertNotIn("--api-key ${{ secrets.", workflow)
        self.assertNotIn("--api-secret ${{ secrets.", workflow)

    def test_release_pr_refresh_handles_both_independent_components(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        script = REFRESH_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("scripts/refresh-release-please-prs.ps1", workflow)
        self.assertIn("release-please--branches--main--components--dcc-cua'", script)
        self.assertIn(
            "release-please--branches--main--components--dcc-cua-browser-extension'",
            script,
        )
        self.assertIn(
            "$expectedBaseCommit = (git rev-parse --verify 'HEAD^{commit}')", script
        )
        self.assertIn(
            'git fetch --no-tags --depth=1 origin "+refs/heads/${baseBranch}:${baseRef}"',
            script,
        )
        self.assertIn("$baseCommit -ne $expectedBaseCommit", script)
        self.assertIn("git checkout -B $branch $baseCommit", script)
        self.assertIn("ManifestKey = '.'", script)
        self.assertIn("ManifestKey = 'browser-extension/chrome'", script)
        self.assertIn("$baseProperty.Value = $componentVersion", script)
        self.assertIn("--force-with-lease", script)
        self.assertNotIn("git merge", script)

    def test_extension_release_cannot_replace_native_latest(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("group: release-${{ github.repository }}", workflow)
        self.assertNotIn("group: release-${{ github.ref }}", workflow)
        self.assertIn("Keep the native runtime release Latest", workflow)
        self.assertIn(
            "steps.release.outputs['browser-extension/chrome--release_created'] == 'true'",
            workflow,
        )
        self.assertIn(
            'gh release edit "$env:EXTENSION_TAG" --repo "$env:GITHUB_REPOSITORY" --latest=false',
            workflow,
        )
        self.assertIn(
            'gh release edit $nativeTag --repo "$env:GITHUB_REPOSITORY" --latest',
            workflow,
        )
        self.assertIn("repository Latest release is not the native runtime", workflow)

        release_job = workflow.index("  release-please:")
        checkout = workflow.index(f"- uses: {CHECKOUT_ACTION}", release_job)
        preexisting_guard = workflow.index(
            "Refuse pre-existing release identities", checkout
        )
        release_action = workflow.index(RELEASE_PLEASE_ACTION, preexisting_guard)
        protection = workflow.index("Keep the native runtime release Latest")
        checkout_section = workflow[checkout:protection]
        self.assertIn("ref: ${{ github.sha }}", checkout_section)

        native_exists = workflow.index("gh release view $nativeTag", protection)
        native_latest = workflow.index("gh release edit $nativeTag", native_exists)
        extension_excluded = workflow.index(
            'gh release edit "$env:EXTENSION_TAG"', native_latest
        )
        final_readback = workflow.index(
            "$latestJson = gh release view", extension_excluded
        )
        final_assertion = workflow.index(
            "$latest.tagName -ne $nativeTag", final_readback
        )
        self.assertLess(checkout, preexisting_guard)
        self.assertLess(preexisting_guard, release_action)
        self.assertLess(release_action, protection)
        self.assertLess(native_exists, native_latest)
        self.assertLess(native_latest, extension_excluded)
        self.assertLess(extension_excluded, final_readback)
        self.assertLess(final_readback, final_assertion)
        protection_section = workflow[protection:final_assertion]
        self.assertEqual(protection_section.count("$LASTEXITCODE -ne 0"), 4)

    def test_release_pr_is_rebuilt_from_main_without_manifest_merge_conflicts(self):
        script = REFRESH_SCRIPT.read_text(encoding="utf-8")

        self.assertNotIn("git merge --no-edit origin/main", script)
        base_fetch = script.index(
            'git fetch --no-tags --depth=1 origin "+refs/heads/${baseBranch}:${baseRef}"'
        )
        base_validation = script.index("$baseCommit -ne $expectedBaseCommit")
        reset = script.index("git checkout -B $branch $baseCommit")
        merge_manifest = script.index("$baseProperty.Value = $componentVersion")
        restore = script.index("& git @restoreArguments")
        synchronize = script.index(
            "& pwsh -NoProfile -File scripts/sync-cargo-workspace-version.ps1 -Version $version"
        )
        guarded_push = script.index('git push "--force-with-lease=$lease"')

        self.assertLess(base_fetch, base_validation)
        self.assertLess(base_validation, reset)
        self.assertLess(reset, restore)
        self.assertLess(reset, merge_manifest)
        self.assertLess(merge_manifest, restore)
        self.assertLess(restore, synchronize)
        self.assertLess(synchronize, guarded_push)

    def test_release_sync_stages_only_release_anchors_and_generated_workspace_files(
        self,
    ):
        script = REFRESH_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("$stageFiles += @('Cargo.toml', 'Cargo.lock')", script)
        self.assertIn("& git add -- @stageFiles", script)
        self.assertNotIn("crates/*/Cargo.toml", script)

    def test_workspace_sync_refreshes_the_lockfile_before_locked_validation(self):
        script = SYNC_SCRIPT.read_text(encoding="utf-8")

        refresh = script.index("cargo metadata --format-version 1 | ConvertFrom-Json")
        locked_validation = script.index(
            "cargo metadata --locked --format-version 1 --no-deps"
        )
        self.assertNotIn("cargo metadata --format-version 1 --no-deps", script)
        self.assertLess(refresh, locked_validation)


if __name__ == "__main__":
    unittest.main()
