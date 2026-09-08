#!/usr/bin/env bash
# Per-boot Docker daemon for Cloud Agent Docker-in-Docker / KIND.
#
# Cursor Cloud VMs use tini as PID 1, so `service docker start` / systemd
# units never run. Start dockerd directly and wait until `docker info`
# succeeds. Do not create a KIND cluster here: that is opt-in per task.
set -euo pipefail

wait_for_docker() {
  local i
  for i in $(seq 1 40); do
    if docker info >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

sudo mkdir -p /var/log /var/run

if [ -S /var/run/docker.sock ] && docker info >/dev/null 2>&1; then
  # Group membership from `usermod -aG docker` does not apply to this session.
  sudo chmod 666 /var/run/docker.sock
  exit 0
fi

if [ -f /var/run/docker.pid ]; then
  oldpid="$(sudo cat /var/run/docker.pid 2>/dev/null || true)"
  if [ -n "${oldpid}" ] && [ -d "/proc/${oldpid}" ]; then
    echo "dockerd already running as pid ${oldpid}" >&2
  else
    sudo rm -f /var/run/docker.pid
  fi
fi

if ! docker info >/dev/null 2>&1; then
  sudo sh -c 'nohup dockerd --host=unix:///var/run/docker.sock >/var/log/dockerd.log 2>&1 &'
fi

if ! wait_for_docker; then
  echo "error: dockerd did not become ready" >&2
  sudo tail -50 /var/log/dockerd.log >&2 || true
  exit 1
fi

sudo chmod 666 /var/run/docker.sock
docker info >/dev/null
echo "dockerd ready ($(docker info --format '{{.ServerVersion}}' 2>/dev/null || echo unknown), $(docker info --format '{{.Driver}}' 2>/dev/null || echo unknown))"
