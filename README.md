# srvcs-floatpower

The floating-point exponentiation primitive of the srvcs.cloud distributed
standard library.

Its single concern: **`base` raised to the power `exp`.** The result is a
floating-point number (`base.powf(exp)`), so unlike the integer primitives this
service can return a fractional value.

It does not validate input itself — it delegates "is this a number" to
[`srvcs-isnumber`](https://github.com/srvcs/isnumber) over HTTP, the single
source of truth for that question, once per operand. Both integer and float
inputs are accepted.

If the computed result is not a real number (`NaN` — for example a negative base
raised to a fractional exponent like `(-8)^0.5`), the service rejects the request
as a domain error (`422`). If `srvcs-isnumber` is unreachable, `srvcs-floatpower`
reports itself **degraded (503)** rather than guessing.

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/` | Service identity, concern, and dependency list |
| `POST` | `/` | Compute `base.powf(exp)` |
| `GET` | `/healthz` `/readyz` `/metrics` `/openapi.json` | srvcs service standard surface |

```sh
curl -s -X POST localhost:8080/ -H 'content-type: application/json' -d '{"base": 2, "exp": 10}'
# {"base":2,"exp":10,"result":1024.0}

curl -s -X POST localhost:8080/ -H 'content-type: application/json' -d '{"base": 2, "exp": 0.5}'
# {"base":2,"exp":0.5,"result":1.4142135623730951}
```

Responses:

- `200 {"base": base, "exp": exp, "result": n}` — evaluated; `result` is an `f64`.
- `422` — an operand is not a number (per `srvcs-isnumber`), or the result is
  not a real number (e.g. negative base with a fractional exponent).
- `503` — a dependency is unavailable.

## Dependencies

- [`srvcs-isnumber`](https://github.com/srvcs/isnumber) — input validation.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SRVCS_BIND_ADDR` | `0.0.0.0:8080` | Bind address |
| `SRVCS_ISNUMBER_URL` | `http://127.0.0.1:8081` | Base URL of `srvcs-isnumber` |
| `SRVCS_ENV` | `development` | Environment label for logs |
| `RUST_LOG` | `info,tower_http=info` | Tracing filter |

## Local checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Orchestration tests stand up a mock `srvcs-isnumber` in-process, so the suite
runs without the rest of the fleet. See
[`srvcs/platform`](https://github.com/srvcs/platform) for the shared standard.

> Note: the `cargoHash` in `flake.nix` is inherited from the template and must be
> refreshed with a `nix build` before the Nix gates pass.
