# SSH + SFTP Workbench

DBX can store an SSH connection as a dedicated data source. Double-clicking the
connection opens a workbench with an interactive terminal on the left and an
SFTP browser on the right.

## Supported authentication

- Password
- Private key file with an optional passphrase
- Private key followed by password
- SSH Agent
- No authentication, when explicitly allowed by the server

Private keys are referenced by path and are never copied into the DBX database.
Passwords and key passphrases use the existing DBX connection secret store.
Host keys use the existing DBX `known_hosts` and trust-on-first-use flow.

## Desktop and Docker

Desktop builds can select a local file for SFTP upload and a local destination
for download. Docker/Web builds upload browser `File` content with multipart
requests. Downloads use a backend-owned transfer task and a bounded 64 KiB
stream connected directly to the browser's native download flow. Transfer
progress and cancellation use the same task ID over an authenticated SSE feed;
the browser does not build a complete in-memory Blob. Browser clients never
send a host filesystem path.

Desktop uploads and downloads are written to a temporary file beside the target
and replace the target only after the transfer completes. Failed and cancelled
transfers remove their temporary file on a best-effort basis. Web downloads are
streamed directly and do not create a server-side destination file.

SSH connections in Docker originate from the container network. The container
must be able to resolve and reach the target host and port. For a server running
on the Docker host, use the platform-appropriate host gateway such as
`host.docker.internal`, or configure an explicit Docker network route.

Private key paths in Docker refer to files inside the container. Mount key files
read-only and restrict their permissions instead of embedding key contents in
the connection record.

## Session behavior

Each workbench tab owns an independent SSH session. The terminal and SFTP
subsystem share that session, tab switches replay any missed terminal output,
and closing the tab releases the session. Restored tabs start disconnected after
an application restart and require an explicit reconnect.

The connection `read_only` flag disables SFTP mutations. It does not restrict
commands entered into the interactive shell. The restriction is enforced in
the shared Core used by both Tauri and Web, not only by disabled UI controls.

Web SSH sessions and transfer tasks are bound to the current DBX login cookie.
When password protection is disabled, only SSH API requests receive an
anonymous HttpOnly owner cookie; existing non-SSH APIs keep their passwordless
behavior. Anonymous SSH owners expire after 24 hours without activity and the
server retains at most 1024 owners. Expiration, eviction, and logout close owned
SSH sessions and cancel their transfers. API clients must preserve and return
the cookie when an SSH workflow spans multiple requests.

SFTP paths are normalized against the remote home directory. `.` and `..`
remain supported for navigation, but normalization cannot escape the remote
filesystem root. DBX does not add a virtual SFTP sandbox: access remains
limited by the permissions of the authenticated SSH account.

## First-stage scope

The first stage includes terminal input/output/resize/reconnect plus SFTP list,
upload, download, create directory, rename, recursive delete, refresh, and
read-only text preview. Server monitoring, AI, rz/sz, archive operations, remote
editing, and port-forward management are intentionally out of scope.
