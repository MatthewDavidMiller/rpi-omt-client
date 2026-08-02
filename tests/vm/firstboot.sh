#!/bin/bash
# One-time provisioning for the isolated Raspberry Pi OS test VM.

set -euo pipefail

user_name=omtvm
authorized_key=/etc/omt-vm-authorized-key
marker=/var/lib/omt-vm/firstboot-complete

if [[ ! -s "${authorized_key}" ]]; then
    echo "VM provisioning key is missing." >&2
    exit 1
fi

if ! id "${user_name}" >/dev/null 2>&1; then
    useradd --create-home --shell /bin/bash "${user_name}"
fi
# An empty password keeps the account unlocked for public-key authentication;
# password SSH is disabled below and sudo accepts no password for this VM user.
passwd --delete "${user_name}"
install -d -m 0700 -o "${user_name}" -g "${user_name}" "/home/${user_name}/.ssh"
install -m 0600 -o "${user_name}" -g "${user_name}" \
    "${authorized_key}" "/home/${user_name}/.ssh/authorized_keys"

printf '%s\n' "${user_name} ALL=(ALL) NOPASSWD: ALL" > "/etc/sudoers.d/${user_name}"
chmod 0440 "/etc/sudoers.d/${user_name}"
visudo --check --file "/etc/sudoers.d/${user_name}"

install -d -m 0755 /etc/ssh/sshd_config.d
cat > /etc/ssh/sshd_config.d/90-omt-vm.conf <<EOF
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
AllowUsers ${user_name}
EOF
ssh-keygen -A
systemctl enable ssh.service

# These generic kernel devices let the installer and Compose boundary run in a
# VM when the Pi OS kernel ships the modules. Their absence is a supported test
# outcome: the installer must defer service startup and explain which real Pi
# devices are missing.
modprobe vkms 2>/dev/null || true
modprobe snd-dummy 2>/dev/null || true

install -d -m 0755 /var/lib/omt-vm
systemctl restart ssh.service
touch "${marker}"
systemctl disable omt-vm-firstboot.service
