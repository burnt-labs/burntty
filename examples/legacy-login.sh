#!/usr/bin/env sh
printf 'login: '
IFS= read -r user
printf 'password: '
stty -echo 2>/dev/null || true
IFS= read -r password
stty echo 2>/dev/null || true
printf '\nlegacy> '

while IFS= read -r command; do
  case "$command" in
    status)
      printf 'user=%s\nOK\nlegacy> ' "$user"
      ;;
    quit|exit)
      printf 'bye\n'
      exit 0
      ;;
    *)
      printf 'unknown: %s\nlegacy> ' "$command"
      ;;
  esac
done
