"""Reject server, database, keyring and forge SDK crates in setup's normal graph.

Input: cargo tree --locked --edges normal --prefix none --format '{p}'.
Dev-only mock HTTP servers are deliberately excluded by --edges normal.
"""
import re
import sys


def forbidden_packages(graph):
    packages = {line.split()[0] for line in graph.splitlines() if line.strip()}
    return sorted(name for name in packages if re.fullmatch(
        r'axum(?:-.*)?|sqlx(?:-.*)?|rusqlite|libsqlite3-sys|'
        r'(?:.*-)?keyring(?:-.*)?|secret-service|zbus(?:-.*)?|'
        r'octocrab|gitlab|forgejo-api|codeberg-api', name
    ))


if __name__ == '__main__':
    graph = sys.stdin.read()
    if not graph.strip():
        sys.exit('Setup dependency graph is empty; refusing an unchecked build.')
    forbidden = forbidden_packages(graph)
    if forbidden:
        sys.exit('Forbidden setup runtime dependencies: ' + ', '.join(forbidden))
    print('Setup runtime dependency boundary passed (no server/SQLite/keyring/forge SDK).')
