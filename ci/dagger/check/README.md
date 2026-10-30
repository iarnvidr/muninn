# Muninn Linux checks

This Dagger 0.21.11 module runs the Rust workspace tests, JavaScript tests, Python packaging tests, updater protocol checks, and publication boundary checks in versioned Linux containers.
Run it with `dagger call -m ci/dagger/check checks --source .`.
