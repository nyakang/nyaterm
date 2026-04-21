---
sidebar_position: 7
---

# Security

Dragonfly's security features mainly focus on three areas:

1. Safely storing local credentials and authentication materials
2. Managing host verification and second-factor flows during SSH login
3. Providing master-password and screen-lock protection in the desktop workspace

## How sensitive local data is stored

Dragonfly stores connection-related configuration locally, but sensitive values are encrypted before being written to disk. Typical sensitive data includes:

- Saved passwords
- SSH private keys and key passphrases
- OTP secrets
- The persisted representation of the master password
- Proxy or other authentication materials that need protection

So in day-to-day use, you work with reusable password, key, and OTP entries rather than scattering plaintext secrets through config files.

## The Security/Auth panel

The **Security/Auth** panel in the left activity bar centralizes authentication-related records into three groups:

- **Keys**
- **Passwords**
- **OTP**

This means you do not need to re-enter every secret in every connection. You save reusable entries first, then reference them from the connection form.

### SSH key management

Good for storing:

- Common login keys
- Keys protected by passphrases
- Multiple identities separated by environment

When you switch an SSH connection to **Private Key** authentication, you can pick from these saved keys directly.

### Password management

Good for storing:

- SSH passwords
- Proxy passwords
- Other credentials you need to reuse

In the SSH connection form, password authentication can reference these saved password entries directly.

### OTP management

OTP management supports:

- **TOTP**
- **HOTP**
- Import from QR code images
- Viewing and generating current codes
- Binding OTP entries to SSH connections

For details, see [OTP & Authentication](./otp-and-auth).

## Master password

The master password is Dragonfly's most important local desktop protection feature.

You can configure it in **Settings → Security**. After it is set:

- The app uses it for unlock verification
- Sensitive local configuration protection is built around it
- The lock screen requires it before unlocking
- **Sync & Backup** actions become eligible to be enabled and used

If you have not set a master password, the lock screen is only a visual lock layer, not full password-based protection.

## Sync & Backup security model

If you plan to use **Settings → Sync & Backup**, the master password is not just recommended. It is a prerequisite.

In the current implementation:

- Sync and backup actions cannot be enabled without a master password
- Cloud-provider credentials are treated as protected local secrets
- Dragonfly uploads **encrypted portable snapshots**, not plain-text config files
- Remote restore overwrites the portable local data included in the snapshot, but it does not blindly roam every piece of device-local UI state to another machine

That makes this feature best understood as:

- **Cross-device sync for portable configuration**
- **Recoverable encrypted backup for a device**

not as a collaborative merge tool.

For the complete setup and workflow details, see [Sync & Backup](./sync-and-backup).

## Screen lock

### Manual lock

You can trigger lock from the UI or by keyboard shortcut at any time.

### Auto lock

In **Settings → Security**, once screen lock is enabled, you can also configure the idle timeout:

- `0` means no idle auto-lock
- Values greater than `0` trigger automatic lock after that many idle minutes

### Unlock behavior

- **With a master password** — entering the correct master password is required
- **Without a master password** — unlocking can happen directly

If you plan to use Dragonfly on a shared machine or during demos, enabling both **master password** and **screen lock** is the safer setup.

## SSH host key policies

When SSH first connects to an unknown host, Dragonfly supports three policies:

| Policy | Behavior |
|------|------|
| Prompt | Ask whether to trust the host key on first connect (default) |
| Accept | Automatically accept new host keys |
| Strict | Reject all unknown host keys |

Known host records are stored in `~/.dragonfly/known_hosts`.

If host identity validation matters in your environment, prefer **Prompt** or **Strict** over unconditional acceptance.

## Practical security advice

- Prefer **private key + OTP** in production environments instead of relying on one password
- Enable both **master password** and **screen lock** on shared computers or demo machines
- Save **jump host** and **proxy** definitions explicitly for environments that depend on them
- Verify the source of a new host key before trusting it

:::tip Screenshot suggestion
- Suggested image path: `/img/docs/security/security-settings.png`
- Show master password, screen lock, idle lock time, and host key policy
- Another good image path: `/img/docs/security/security-auth-panel.png`
- Show the Keys, Passwords, and OTP tabs in the Security/Auth panel
:::
