from __future__ import annotations

from pathlib import Path

import yaml


def test_wheel_build_uses_the_python_interpreter_from_uv_project_environment() -> None:
    workflow = yaml.safe_load(Path(".github/workflows/wheels.yml").read_text(encoding="utf-8"))
    steps = workflow["jobs"]["build"]["steps"]
    environment_step = next(step["run"] for step in steps if "UV_PROJECT_ENVIRONMENT=${RUNNER_TEMP}/radiust-venv" in step.get("run", ""))
    build_step = next(step["run"] for step in steps if "maturin build --release" in step.get("run", ""))

    assert environment_step
    assert '--interpreter "$UV_PROJECT_ENVIRONMENT/bin/python"' in build_step
    assert ".venv/bin/python" not in build_step


def test_build_matrix_smoke_uses_only_its_own_wheel_artifact() -> None:
    workflow = yaml.safe_load(Path(".github/workflows/wheels.yml").read_text(encoding="utf-8"))
    steps = workflow["jobs"]["build"]["steps"]
    build_index = next(index for index, step in enumerate(steps) if "maturin build --release" in step.get("run", ""))
    selection_index = next(
        (index for index, step in enumerate(steps) if "RADIUST_WHEEL=" in step.get("run", "")),
        None,
    )
    smoke_index = next(index for index, step in enumerate(steps) if "pytest tests/packaging -v" in step.get("run", ""))
    wheel_dir = "validation-results/wheels/${{ matrix.os }}-${{ matrix.python }}-${{ matrix.architecture }}"
    build_step = steps[build_index]["run"]
    upload = next(step for step in steps if step.get("uses", "").startswith("actions/upload-artifact@"))

    assert '--out "$WHEEL_DIR"' in build_step
    assert steps[build_index]["env"]["WHEEL_DIR"] == wheel_dir
    assert selection_index is not None, "the smoke must receive the wheel built by this matrix cell"
    assert build_index < selection_index < smoke_index
    assert steps[selection_index]["env"]["WHEEL_DIR"] == wheel_dir
    assert wheel_dir in upload["with"]["path"]


def test_extra_matrix_downloads_and_smokes_its_own_wheel_artifact() -> None:
    workflow = yaml.safe_load(Path(".github/workflows/wheels.yml").read_text(encoding="utf-8"))
    steps = workflow["jobs"]["extras"]["steps"]
    download = next(step for step in steps if step.get("uses", "").startswith("actions/download-artifact@"))
    selection_index = next(
        (index for index, step in enumerate(steps) if "RADIUST_WHEEL=" in step.get("run", "")),
        None,
    )
    smoke_index = next(
        index for index, step in enumerate(steps) if "pytest tests/packaging/test_installed_wheel.py" in step.get("run", "")
    )
    wheel_dir = "validation-results/wheels/${{ matrix.python }}-${{ matrix.extra }}"

    assert selection_index is not None, "each extra smoke must receive its downloaded wheel explicitly"
    assert wheel_dir in download["with"]["path"]
    assert steps[selection_index]["env"]["WHEEL_DIR"] == wheel_dir
    assert selection_index < smoke_index


def test_extra_job_installs_selected_wheel_before_repository_pytest_bootstrap() -> None:
    workflow = yaml.safe_load(Path(".github/workflows/wheels.yml").read_text(encoding="utf-8"))
    steps = workflow["jobs"]["extras"]["steps"]
    sync_index = next(index for index, step in enumerate(steps) if "uv sync --no-dev --group ci --locked --no-install-project" in step.get("run", ""))
    install_index = next(
        (index for index, step in enumerate(steps) if "uv pip install" in step.get("run", "")),
        None,
    )
    smoke_index = next(
        index for index, step in enumerate(steps) if "pytest tests/packaging/test_installed_wheel.py" in step.get("run", "")
    )

    assert install_index is not None, "pytest's repository conftest requires the selected wheel to be importable"
    install_step = steps[install_index]["run"]
    assert sync_index < install_index < smoke_index
    assert "--no-deps" in install_step
    assert "$RADIUST_WHEEL" in install_step
    assert '--python "$UV_PROJECT_ENVIRONMENT/bin/python"' in install_step
