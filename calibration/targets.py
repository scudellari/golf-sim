"""Read calibration measurements from the validation corpus and its licence ledger."""
import json
import shutil
import tomllib
from pathlib import Path
from urllib.request import urlretrieve
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parent.parent
VALIDATION = ROOT / "testdata/validation"


def fit_measurements(row):
    if row["use"] != "fit":
        return {}
    held_out = row.get("held_out", ())
    return {name: value for name, value in row["measured"].items() if name not in held_out}


class Targets:
    def __init__(self, scratch):
        ledger = VALIDATION / "targets/LICENCES.toml"
        with ledger.open("rb") as source:
            self.datasets = tomllib.load(source)["datasets"]
        self.directory = VALIDATION
        remote = [data for data in self.datasets.values() if data["verdict"] == "fetch"]
        if remote:
            self.directory = scratch / "validation"
            shutil.copytree(VALIDATION, self.directory)
            shutil.copyfile(VALIDATION.with_name("tolerances.toml"),
                            self.directory.with_name("tolerances.toml"))
            for index, dataset in enumerate(remote):
                self._fetch(dataset, scratch / f"download-{index}")

    def _fetch(self, dataset, download):
        files = dataset["files"]
        if not files:
            raise ValueError("A fetched dataset needs ledger file paths")
        target_dir = (self.directory / "targets").resolve()
        destinations = [target_dir / name for name in files]
        for path in destinations:
            if not path.resolve().is_relative_to(target_dir):
                raise ValueError(f"Invalid fetched file path {path}")
            path.parent.mkdir(parents=True, exist_ok=True)
        urlretrieve(dataset["fetch"]["url"], download)
        if len(files) == 1:
            shutil.copyfile(download, destinations[0])
        else:
            with ZipFile(download) as archive:
                for name, path in zip(files, destinations):
                    with archive.open(name) as source, path.open("wb") as output:
                        shutil.copyfileobj(source, output)

    def files(self, dataset):
        data = self.datasets[dataset]
        if data["verdict"] == "excluded":
            return
        for name in data.get("files", ()):
            path = self.directory / "targets" / name
            if path.is_file() or data["verdict"] != "fetch":
                yield path

    def rows(self, sources, kind):
        files = {path for name, data in self.datasets.items()
                 if set(sources).intersection(data.get("sources", ()))
                 for path in self.files(name) if path.suffix == ".json"}
        rows = []
        for path in sorted(files):
            file = json.loads(path.read_text(encoding="utf-8"))
            if file["source"] in sources:
                rows.extend(row for row in file["targets"] if row["kind"] == kind)
        return sorted(rows, key=lambda row: row["id"])
