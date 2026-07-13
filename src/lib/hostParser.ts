/**
 * Parse `user@host[:port]` format input from the Host field.
 *
 * Returns parsed components when the input matches the pattern,
 * or `null` when no `@` is present (normal hostname input).
 *
 * Supported formats:
 *   user@host
 *   user@host:port
 *   user@[ipv6]
 *   user@[ipv6]:port
 *
 * Rejected (returns null, caller should use raw value):
 *   user:password@host  (inline password, security risk)
 *   @host               (empty username)
 *   user@               (empty host)
 */
export interface ParsedHostInput {
  host: string;
  username: string;
  port?: number;
}

export function parseUserHostInput(value: string): ParsedHostInput | null {
  const trimmed = value.trim();
  if (!trimmed) return null;

  const atIndex = trimmed.lastIndexOf("@");
  if (atIndex <= 0 || atIndex === trimmed.length - 1) {
    return null;
  }

  const userPart = trimmed.slice(0, atIndex);
  const hostPart = trimmed.slice(atIndex + 1);

  // Reject inline password: user:password@host
  if (userPart.includes(":")) {
    return null;
  }

  if (!userPart || !hostPart) {
    return null;
  }

  const { host, port } = parseHostPort(hostPart);

  return {
    host,
    username: userPart,
    port: port ?? undefined,
  };
}

/**
 * Parse host[:port] or [ipv6][:port] from the host portion.
 */
function parseHostPort(target: string): { host: string; port: number | null } {
  // IPv6 bracketed: [::1] or [::1]:22
  if (target.startsWith("[")) {
    const end = target.indexOf("]");
    if (end === -1) {
      return { host: target, port: null };
    }
    const host = target.slice(0, end + 1); // keep brackets
    const rest = target.slice(end + 1);
    if (rest.startsWith(":")) {
      const port = Number(rest.slice(1));
      return { host, port: Number.isFinite(port) ? port : null };
    }
    return { host, port: null };
  }

  // Regular host:port — only split on the last colon
  const colonCount = (target.match(/:/g) ?? []).length;
  if (colonCount === 1) {
    const [host, portText] = target.split(":");
    const port = portText ? Number(portText) : null;
    return { host, port };
  }

  // No port (or ambiguous multiple colons without brackets — treat as raw host)
  return { host: target, port: null };
}
