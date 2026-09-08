#!/usr/bin/env bash
# Per-boot Docker daemon for Cloud Agent Docker-in-Docker / KIND.
#
# Cursor Cloud VMs use tini as PID 1, so `service docker start` / systemd
# units never run. Start dockerd directly and wait until the daemon answers.
# Do not create a KIND cluster here: that is opt-in per task.
set -euo pipefail

docker_ready() {
  sudo docker info >/dev/null 2>&1
}

wait_for_docker() {
  local i
  for i in $(seq 1 40); do
    if docker_ready; then
      return 0
    fi
    sleep 1
  done
  return 1
}

expose_docker_socket() {
  # Group membership from `usermod -aG docker` does not apply to this session.
  sudo chmod 666 /var/run/docker.sock
}

sudo mkdir -p /var/log /var/run

if docker_ready; then
  expose_docker_socket
  docker info >/dev/null
  echo "dockerd already ready ($(docker info --format '{{.ServerVersion}}'), $(docker info --format '{{.Driver}}'))"
  exit 0
fi

if [ -f /var/run/docker.pid ]; then
  oldpid="$(sudo cat /var/run/docker.pid 2>/dev/null || true)"
  if [ -n "${oldpid}" ] && [ -d "/proc/${oldpid}" ]; then
    echo "dockerd pid ${oldpid} is running; waiting for the API" >&2
    if wait_for_docker; then
      expose_docker_socket
      docker info >/dev/null
      echo "dockerd ready ($(docker info --format '{{.ServerVersion}}'), $(docker info --format '{{.Driver}}'))"
      exit 0
    fi
    echo "error: dockerd pid ${oldpid} did not become ready" >&2
    sudo tail -50 /var/log/dockerd.log >&2 || true
    exit 1
  fi
  sudo rm -f /var/run/docker.pid
fi

sudo sh -c 'nohup dockerd --host=unix:///var/run/docker.sock >/var/log/dockerd.log 2>&1 &'

if ! wait_for_docker; then
  echo "error: dockerd did not become ready" >&2
  sudo tail -50 /var/log/dockerd.log >&2 || true
  exit 1
fi

expose_docker_socket
docker info >/dev/null
echo "dockerd ready ($(docker info --format '{{.ServerVersion}}'), $(docker info --format '{{.Driver}}'))"
