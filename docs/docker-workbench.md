# Docker Workbench

DBX can manage an existing Docker Engine from both the Tauri desktop application and the DBX Web deployment. Docker connections reuse DBX connection groups, cloud sync metadata, production/read-only markers, and shared SSH transport profiles.

## Supported transports

| Protocol | Target | Requirements |
| --- | --- | --- |
| HTTP | Docker Engine TCP API, normally `127.0.0.1:2375` | Remote clear-text HTTP is blocked unless explicitly enabled. Prefer an SSH transport. |
| HTTPS | Docker Engine TLS API, normally port `2376` | CA path and optional client certificate/private-key paths must be readable by the DBX backend. |
| Unix | A local socket such as `/var/run/docker.sock` | The DBX backend must run on Unix and have socket permission. Windows named pipes are not supported. |
| Unix-Over-Nc | A Unix socket on an SSH host | Select exactly one SSH profile; the remote host must provide `nc -U`. |
| Unix-Over-Nc-Sudo | A privileged Unix socket on an SSH host | The remote host must allow `sudo -n -- nc -U ...` without a password. Interactive sudo is intentionally unsupported. |

API version `auto` discovers the Engine version through `/version`. Docker API versions older than 1.24 are rejected.

## Secure Web deployment

The default DBX Compose deployment does **not** mount the Docker socket. Mount it only when the DBX Web instance is trusted and access-controlled:

```yaml
services:
  dbx:
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
```

Access to the Docker socket is root-equivalent. Any user who can control Docker can mount host paths, access host data, and start privileged workloads. DBX therefore rejects start, stop, and restart operations when Web password protection is disabled (`DBX_DISABLE_PASSWORD=true`). A connection marked read-only also rejects these operations in the shared backend core.

For a remote TLS Engine, mount certificates read-only and configure their **backend file paths** in the connection:

```yaml
services:
  dbx:
    volumes:
      - ./docker-certs:/run/secrets/docker:ro
```

Example paths:

- CA: `/run/secrets/docker/ca.pem`
- Client certificate: `/run/secrets/docker/cert.pem`
- Client private key: `/run/secrets/docker/key.pem`

The Web connection form does not upload certificate files from the browser.

## SSH NC setup

Verify the remote prerequisites using the same SSH account configured in DBX:

```sh
command -v nc
printf 'GET /_ping HTTP/1.0\r\n\r\n' | nc -U /var/run/docker.sock
sudo -n -- true
```

For Sudo-NC, grant only the command and socket path required by your environment. DBX never sends an interactive sudo password.

## First-version scope

The workbench provides read-only lists for containers, images, volumes, and networks; container start/stop/restart; container summary; session-only 15-minute trends; and raw Inspect JSON. It does not invoke the Docker or Compose CLI. Container creation/deletion, image pulls, logs, exec terminals, Compose lifecycle operations, Windows named pipes, and persistent monitoring are outside this version.
