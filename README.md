# Kickstart

Kickstart automates the process of starting and orchestrating multi-process local development environments. Define your services, their readiness conditions, and dependencies declaratively in your `kickstart.toml`:

```toml
[services]

[services.backend]
command = "python3 -u backend.py"
ready_condition = { port = 8000 }

[services.seed]
command = "python3 -u seed.py"
depends_on = ["backend"]
ready_condition = { exitcode = 0 }

[services.frontend]
command = "python3 -u frontend.py"
depends_on = ["backend"]
ready_condition = { port = 3000 }
```

Then run:

```sh
$ kickstart up
```

Kickstart will create a DAG from your configuration and execute each service in dependency order by traversing the graph. Once all services are ready, kickstart will notify you.

<p align="center">
<img width="800" height="458" alt="kickstart" src="https://github.com/user-attachments/assets/ed107c27-10ee-440c-9f39-b191ec9d586c" />
</p>

## Installing

Clone the repo, then install with `cargo install`:

```sh
$ git clone https://github.com/rk234/kickstart
$ cd kickstart
$ cargo install --path .
```

## Commands

Configuration is loaded from the current working directory by default. You can specify an alternative config file with the `--config` flag. There are two commands:

```
kickstart services [--json]
```

Lists services in a table including their start stage and dependencies. The `--json` flag serializes the output as a JSON object for easy parsing by other tools. When configuration is invalid, validation errors are outputted as well.

```
kickstart up
```

Starts services in dependency order. Each service's stdout and stderr are included in the command output with labeled by their corresponding service ID. Gracefully exits each service on SIGTERM.

## Configuration

Configuring a service is simple:
```
[services.{service-id}]
name = "human readable name"
command = "what to execute"
depends_on = [{list other valid service IDs here}]
ready_condition = { port = 3000 } | "immediate" | {exitcode = 0} | {delay = 1000(ms)}
```

### Ready conditions

Kickstart supports a variety of readiness conditions. Once a service is marked ready its dependents are executed.

- Immediate: Service is immediately marked ready after it starts
- Port: Service is marked ready when the specified port begins accepting connections
- Exit code: Service is marked ready when it exits with the specified exit code
- Delay: Service is marked ready after the specified delay milliseconds pass since starting

### Environment variables

Ready condition values can reference environment variables with `{VAR}` or `{VAR:-default}`:

```toml
ready_condition = { port = "{BACKEND_PORT:-8000}" }
```

Kickstart loads `.env` from the current directory by default if it exists, or a different file passed with `--env`. Variables already set in your shell take precedence over the file. Loaded variables are also passed to each service's command.

## Examples

See the `examples/basic` for a basic example config, and `examples/env` for the same setup with its ports set in a `.env` file, used both in the ready conditions and by the services themselves.
