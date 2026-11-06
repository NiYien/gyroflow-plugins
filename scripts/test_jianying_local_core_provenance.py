# SPDX-License-Identifier: GPL-3.0-or-later
import copy
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import jianying_local_core_provenance as provenance
import test_jianying_macos_candidate as candidate_tests

candidate = candidate_tests.candidate


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def entries(root):
    return {path.relative_to(root).as_posix(): {"sha256": sha(path), "size": path.stat().st_size}
            for path in sorted(root.rglob("*")) if path.is_file()}


class SourceFixture:
    def __init__(self, root):
        self.root = Path(root)
        self.repo = self.root / "authority"
        self.repo.mkdir()
        baseline = {"src/core/Cargo.toml": '[package]\nname="gyroflow-core"\nversion="1.6.3"\n',
                    "src/core/stabilization/mod.rs": "pub const DEFAULT_FLAGS: u32 = 0;\n",
                    "src/core/gpu/wgpu_undistort.wgsl": "// Baseline shader fixture.\n",
                    "src/core/gpu/mod.rs": "pub const EXISTING_FLAGS: u32 = 8192;\n",
                    "src/core/gpu/stabilize_spirv/Cargo.toml": '[package]\nname="stabilize_spirv"\nversion="0.0.0"\n',
                    "vendor/neuflow-trace/neuflow-trace-shared/Cargo.toml": '[package]\nname="neuflow-trace-shared"\nversion="0.1.0"\n',
                    "vendor/neuflow-trace/neuflow-trace-shared/src/lib.rs": "pub fn shared_fixture() {}\n"}
        for name, value in baseline.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
        self.git("init", "--quiet")
        self.git("add", ".")
        self.git("-c", "user.name=SourceFixture", "-c", "user.email=source-fixture@example.invalid",
                 "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "--no-verify", "-qm", "base")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.core = self.repo / "src/core"
        changed = ["src/core/stabilization/mod.rs", "src/core/gpu/wgpu_undistort.wgsl"]
        self.modified = [{"path": name, "base_sha256": sha(self.repo / name)} for name in changed]
        for name in changed:
            with (self.repo / name).open("a") as stream:
                stream.write("// Explicit default-off local opt-in fixture.\n")
        for item in self.modified:
            item["sha256"] = sha(self.repo / item["path"])
        self.core_snapshot = self.root / "snapshot-core"
        shutil.copytree(self.core, self.core_snapshot)
        self.dep = self.repo / "vendor/neuflow-trace/neuflow-trace-shared"
        self.dep_snapshot = self.root / "snapshot-neuflow"
        shutil.copytree(self.dep, self.dep_snapshot)
        self.formal = self.root / "formal-plugin"
        (self.formal / "common").mkdir(parents=True)
        (self.formal / "Cargo.toml").write_text("[workspace]\nmembers=[]\n")
        (self.formal / "common/Cargo.toml").write_text('gyroflow-core = { git="https://github.com/NiYien/gyroflow.git", rev="' + self.base + '" }\n')
        self.workspace = self.root / "isolated-workspace"
        self.workspace.mkdir()
        (self.workspace / "Cargo.toml").write_text('[patch."https://github.com/NiYien/gyroflow.git"]\ngyroflow-core={path="' + str(self.core) + '"}\n')
        names = [("gyroflow-core", "1.6.3"), ("neuflow-trace-shared", "0.1.0"), ("stabilize_spirv", "0.0.0")]
        source = "git+https://github.com/NiYien/gyroflow.git?rev=" + self.base + "#" + self.base
        removed = [{"name": name, "version": version, "source": source} for name, version in names]
        added = [{"name": name, "version": version} for name, version in names]
        unchanged = {"name": "unchanged-registry-crate", "version": "1.0.0", "source": "registry+https://example.invalid/index", "checksum": "a" * 64}
        self.write_lock(self.formal / "Cargo.lock", removed + [unchanged])
        self.write_lock(self.workspace / "Cargo.lock", added + [unchanged])
        manifests = {"gyroflow-core": self.core / "Cargo.toml", "stabilize_spirv": self.core / "gpu/stabilize_spirv/Cargo.toml",
                     "neuflow-trace-shared": self.dep / "Cargo.toml"}
        resolved = [{"name": name, "version": version, "manifest_path": str(manifests[name]), "source": None} for name, version in names]
        self.metadata_path = self.root / "cargo-metadata.json"
        write_json(self.metadata_path, {"workspace_root": str(self.workspace), "packages": resolved})
        self.binary = self.workspace / "libgyroflow_ofx.dylib"
        self.binary.write_bytes(b"Synthetic inner-binary identity fixture; not executable.")
        self.gpu_path = self.root / "gpu-summary.json"
        self.gpu_summary = {"schema": 1, "kind": "macos-dual-metal-buffer-pixel-center-validation", "passed": True,
                            "failures": 0, "default_off_equivalent": True, "identity_bit_exact": True,
                            "roll_bit_exact": False, "opt_in": {"bit": 16384, "default": False},
                            "source_sha256": {name: sha(self.repo / name) for name in
                                ("src/core/stabilization/mod.rs", "src/core/gpu/wgpu_undistort.wgsl", "src/core/gpu/mod.rs")}}
        self.gpu_summary["precision_tolerance"] = {
            "roll_total_mae_u8_limit": 0.5, "roll_every_frame_mae_u8_limit": 0.5,
            "roll_min_fraction_channels_abs_error_le_one": 0.9999, "coefficient_phases": 32,
            "max_neighbor_coefficient_phase_delta": 1, "phase_boundary_distance_limit_units": 0.01,
            "unorm_rounding_allowance_u8": 1, "identity_and_default_off_tolerance": "strict bit equality",
            "every_gt_one_channel_must_fit_independent_envelope": True}
        self.gpu_summary["checks"] = {key: True for key in (
            "default_off_formal_e9_gpu_bit_exact", "optin_identity_integer_cpu_bit_exact",
            "roll_total_mae_u8_le_0p5", "roll_every_frame_mae_u8_le_0p5",
            "roll_at_least_99p99_percent_channels_abs_le_one", "roll_every_gt_one_channel_independent_phase_envelope",
            "roll_every_gt_one_point_at_phase_boundary", "all_gpu_calls_backend_wgpu",
            "cache_optin_and_dimension_separation", "time_matrix_direction_invariance")}
        self.gpu_summary["roll_actual"] = {"total_mae_u8": 0.25, "max_frame_mae_u8": 0.3,
            "fraction_channels_abs_error_le_one": 0.99999, "max_nearest_axis_phase_boundary_distance_units": 0.005,
            "independent_value_envelope_failures": 0, "max_channel_error_u8": 11, "bit_exact": False}
        self.gpu_summary["evidence"] = {}
        for name in ("build_provenance", "full_gpu_and_cache_results", "independent_phase_envelope", "independent_envelope_script"):
            path = self.root / (name + ".json")
            path.write_text('{"scope":"synthetic unit-test receipt; no GPU execution"}\n')
            self.gpu_summary["evidence"][name] = {"path": str(path), "sha256": sha(path)}
        write_json(self.gpu_path, self.gpu_summary)
        core_manifest, core_tree = self.manifest(self.core_snapshot, "core-manifest.json")
        dep_manifest, dep_tree = self.manifest(self.dep_snapshot, "dep-manifest.json")
        self.data = {"schema": 1, "kind": "local-platform-patch", "base_core_revision": self.base,
                     "base_core_tree": self.git("rev-parse", self.base + ":src/core").strip(),
                     "authoritative_repo": str(self.repo), "repo_head": self.base, "files": self.modified,
                     "opt_in": {"bit": 16384, "name": "INTEGER_FRAGMENT_COORDINATES", "default": False,
                                "platform": "macos", "required_sources": ["MetalBuffer", "MetalBuffer"]},
                     "source_snapshot": {"root": str(self.core_snapshot), "manifest_path": str(core_manifest),
                                         "manifest_sha256": sha(core_manifest), "tree_sha256": core_tree},
                     "isolated_build": {"workspace": str(self.workspace), "cargo_lock_path": str(self.workspace / "Cargo.lock"),
                         "cargo_lock_sha256": sha(self.workspace / "Cargo.lock"), "workspace_cargo_toml_sha256": sha(self.workspace / "Cargo.toml"),
                         "formal_plugin_cargo_toml_sha256": sha(self.formal / "common/Cargo.toml"),
                         "formal_plugin_lock_sha256": sha(self.formal / "Cargo.lock"),
                         "formal_workspace_cargo_toml_sha256": sha(self.formal / "Cargo.toml"),
                         "metadata_path": str(self.metadata_path), "metadata_sha256": sha(self.metadata_path),
                         "resolved_core_manifest_path": str(self.core / "Cargo.toml"), "source": None,
                         "resolved_local_packages": resolved,
                         "lock_source_identity_difference": {"removed": removed, "added": added}},
                     "local_dependency_sources": [{"name": "neuflow-trace-shared", "root": str(self.dep),
                         "base_git_path": "vendor/neuflow-trace/neuflow-trace-shared", "snapshot_root": str(self.dep_snapshot),
                         "manifest_path": str(dep_manifest), "manifest_sha256": sha(dep_manifest), "tree_sha256": dep_tree,
                         "files": [{"path": name, "sha256": item["sha256"], "base_sha256": item["sha256"]}
                                   for name, item in entries(self.dep).items()]}],
                     "inner_binary": {"path": str(self.binary), "sha256": sha(self.binary)},
                     "gpu_validation": {"summary_path": str(self.gpu_path), "summary_sha256": sha(self.gpu_path), "result": "passed"}}
        self.receipt = self.root / "provenance.json"
        self.save()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], text=True)

    def manifest(self, folder, name):
        path = self.root / name
        body = json.dumps(entries(folder), sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        path.write_bytes(body)
        return path, hashlib.sha256(body).hexdigest()

    @staticmethod
    def write_lock(path, packages):
        path.write_text("version = 4\n\n" + "\n".join("[[package]]\n" + "\n".join(key + " = " + json.dumps(value)
            for key, value in package.items()) + "\n" for package in packages))

    def save(self):
        write_json(self.receipt, self.data)

    def validate(self, binary=None):
        self.save()
        with patch.object(provenance, "BASE_REVISION", self.base):
            return provenance.validate_local_core_provenance(self.receipt, binary or self.binary, self.formal)


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.fixture = SourceFixture(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def test_complete_local_source_identity_is_verified(self):
        result = self.fixture.validate()
        self.assertEqual(result["formal_dependency_revision"], self.fixture.base)
        self.assertEqual(result["actual_core_source"]["kind"], "local-platform-patch")
        self.assertEqual(result["local_core_provenance_sha256"], sha(self.fixture.receipt))
        self.assertNotIn("shared_core_source_revision", result)
        self.assertEqual(result["actual_core_source"]["gpu_validation"]["summary"]["roll_actual"]["max_channel_error_u8"], 11)
        self.assertFalse(result["actual_core_source"]["gpu_validation"]["summary"]["roll_bit_exact"])

    def test_wrong_base_flag_or_missing_evidence_is_rejected(self):
        original = copy.deepcopy(self.fixture.data)
        changes = [("base", lambda d: d.update(base_core_tree="0" * 40)),
                   ("kind", lambda d: d.update(kind="formal-git-dependency")),
                   ("flag", lambda d: d["opt_in"].update(bit=8192)),
                   ("default", lambda d: d["opt_in"].update(default=True)),
                   ("pending", lambda d: d["gpu_validation"].update(result="pending")),
                   ("missing", lambda d: d.pop("source_snapshot")),
                   ("file-sha", lambda d: d["files"][0].update(sha256="0" * 64)),
                   ("lock-table", lambda d: d["isolated_build"]["lock_source_identity_difference"]["removed"].pop())]
        for name, change in changes:
            with self.subTest(name=name):
                self.fixture.data = copy.deepcopy(original)
                change(self.fixture.data)
                with self.assertRaises(ValueError):
                    self.fixture.validate()

    def test_real_authority_and_snapshot_tampering_is_rejected(self):
        path = self.fixture.core / "gpu/mod.rs"
        old = path.read_bytes()
        path.write_bytes(old + b"// unrecorded authority change\n")
        with self.assertRaisesRegex(ValueError, "inventories differ"):
            self.fixture.validate()
        path.write_bytes(old)
        (self.fixture.core_snapshot / "gpu/mod.rs").write_bytes(old + b"// unrecorded snapshot change\n")
        with self.assertRaisesRegex(ValueError, "inventories differ"):
            self.fixture.validate()

    def test_refreshed_metadata_cannot_hide_wrong_resolution_or_lock_drift(self):
        metadata = json.loads(self.fixture.metadata_path.read_text())
        metadata["packages"][0]["source"] = "git+unexpected"
        write_json(self.fixture.metadata_path, metadata)
        self.fixture.data["isolated_build"]["metadata_sha256"] = sha(self.fixture.metadata_path)
        with self.assertRaisesRegex(ValueError, "does not resolve"):
            self.fixture.validate()
        metadata["packages"][0]["source"] = None
        write_json(self.fixture.metadata_path, metadata)
        self.fixture.data["isolated_build"]["metadata_sha256"] = sha(self.fixture.metadata_path)
        lock = self.fixture.workspace / "Cargo.lock"
        lock.write_text(lock.read_text().replace('version = "1.0.0"', 'version = "2.0.0"'))
        self.fixture.data["isolated_build"]["cargo_lock_sha256"] = sha(lock)
        with self.assertRaisesRegex(ValueError, "names or versions"):
            self.fixture.validate()

    def test_refreshed_gpu_receipt_requires_true_pass_and_matching_sources(self):
        original = copy.deepcopy(self.fixture.gpu_summary)
        for key, value in (("passed", False), ("failures", 1), ("default_off_equivalent", False)):
            with self.subTest(key=key):
                summary = copy.deepcopy(original)
                summary[key] = value
                write_json(self.fixture.gpu_path, summary)
                self.fixture.data["gpu_validation"]["summary_sha256"] = sha(self.fixture.gpu_path)
                with self.assertRaisesRegex(ValueError, "has not passed"):
                    self.fixture.validate()
        summary = copy.deepcopy(original)
        summary["source_sha256"]["src/core/gpu/mod.rs"] = "0" * 64
        write_json(self.fixture.gpu_path, summary)
        self.fixture.data["gpu_validation"]["summary_sha256"] = sha(self.fixture.gpu_path)
        with self.assertRaisesRegex(ValueError, "GPU validation source differs"):
            self.fixture.validate()

    def test_external_dependency_or_binary_bytes_cannot_drift(self):
        old = self.fixture.binary.read_bytes()
        self.fixture.binary.write_bytes(old + b" changed")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.fixture.validate()
        self.fixture.binary.write_bytes(old)
        other = self.fixture.root / "different-binary.dylib"
        other.write_bytes(old)
        with self.assertRaisesRegex(ValueError, "differs from --core"):
            self.fixture.validate(other)
        (self.fixture.dep / "src/lib.rs").write_text("changed dependency\n")
        with self.assertRaisesRegex(ValueError, "inventories differ"):
            self.fixture.validate()

    def test_precision_gate_check_and_real_evidence_tampering_is_rejected(self):
        original = copy.deepcopy(self.fixture.gpu_summary)
        changes = [("tolerance", lambda d: d["precision_tolerance"].update(roll_total_mae_u8_limit=1.0)),
                   ("check", lambda d: d["checks"].update(time_matrix_direction_invariance=False)),
                   ("actual-mae", lambda d: d["roll_actual"].update(total_mae_u8=0.6)),
                   ("phase", lambda d: d["roll_actual"].update(max_nearest_axis_phase_boundary_distance_units=0.02)),
                   ("bit-exact-claim", lambda d: d.update(roll_bit_exact=True))]
        for name, change in changes:
            with self.subTest(name=name):
                summary = copy.deepcopy(original)
                change(summary)
                write_json(self.fixture.gpu_path, summary)
                self.fixture.data["gpu_validation"]["summary_sha256"] = sha(self.fixture.gpu_path)
                with self.assertRaises(ValueError):
                    self.fixture.validate()
        write_json(self.fixture.gpu_path, original)
        self.fixture.data["gpu_validation"]["summary_sha256"] = sha(self.fixture.gpu_path)
        evidence = Path(original["evidence"]["independent_phase_envelope"]["path"])
        evidence.write_text("unrecorded envelope change\n")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.fixture.validate()

    def test_cold_archive_verification_never_reads_declared_local_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            helper = candidate_tests.CandidateTests()
            root, delivery = helper.fixture(directory)
            bundle = root / candidate.BUNDLE
            resources = bundle / "Contents/Resources"
            data = copy.deepcopy(self.fixture.data)
            data["base_core_revision"] = provenance.BASE_REVISION
            core = bundle / "Contents/MacOS" / candidate.CORE
            data["inner_binary"]["sha256"] = sha(core)
            for record in (data,):
                record["authoritative_repo"] = "/not-present/cold-machine/authority"
            raw = json.dumps(data, sort_keys=True).encode()
            (resources / "local-core-provenance.json").write_bytes(raw)
            gpu_raw = self.fixture.gpu_path.read_bytes()
            (resources / "local-core-gpu-validation.json").write_bytes(gpu_raw)
            fields = {"formal_dependency_revision": provenance.BASE_REVISION,
                      "actual_core_source": {"kind": "local-platform-patch", "base_revision": provenance.BASE_REVISION,
                                             "base_tree": data["base_core_tree"],
                                             "tree_sha256": data["source_snapshot"]["tree_sha256"],
                                             "gpu_validation": {"summary": self.fixture.gpu_summary,
                                                                "summary_sha256": data["gpu_validation"]["summary_sha256"]}},
                      "core_sha256": sha(core), "local_core_provenance": data,
                      "local_core_provenance_sha256": hashlib.sha256(raw).hexdigest(),
                      "local_core_provenance_file": provenance.PROVENANCE_RESOURCE,
                      "local_core_gpu_validation_file": provenance.GPU_VALIDATION_RESOURCE}
            inner_path = resources / "package-manifest.json"
            inner = json.loads(inner_path.read_text())
            inner.update(fields)
            inner["files"] = [item for item in candidate.file_inventory(bundle)
                              if item["path"] not in ("Contents/Resources/package-manifest.json", "Contents/MacOS/" + candidate.WRAPPER)
                              and not item["path"].startswith("Contents/_CodeSignature/")]
            write_json(inner_path, inner)
            delivery.update(fields)
            delivery["files"] = [{**item, "path": candidate.BUNDLE + "/" + item["path"]}
                                 for item in candidate.file_inventory(bundle)]
            write_json(root / candidate.DELIVERY, delivery)
            package = helper.archive(root)
            with patch.object(candidate, "local_validation_module", return_value=provenance), \
                    patch.object(provenance, "git", side_effect=AssertionError("Archive verifier called Git")), \
                    patch.object(provenance, "absolute", side_effect=AssertionError("Archive verifier read a local path")), \
                    patch.object(provenance, "checked_file", side_effect=AssertionError("Archive verifier read a local file")):
                self.assertEqual(candidate.verify_delivery(package)["actual_core_source"]["kind"], "local-platform-patch")


if __name__ == "__main__":
    unittest.main()
