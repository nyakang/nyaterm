import type { CommandRiskResponse, RiskLevel } from "@/types/global";

type RiskPattern = {
  pattern: RegExp;
  riskLevel: RiskLevel;
  blocked: boolean;
  reason: string;
  safeAlternatives: string[];
  confirmText?: string;
};

const patterns: RiskPattern[] = [
  {
    pattern: /(^|[;&|]\s*)rm\s+-[^\n]*r[^\n]*f[^\n]*\s+(\/|\/\*|--no-preserve-root\s+\/)(\s|$)/i,
    riskLevel: "critical",
    blocked: true,
    reason: "该命令可能递归删除根目录或根目录下的大量文件，风险不可恢复。",
    safeAlternatives: ["ls -lah /", "find / -maxdepth 1 -mindepth 1 -print | head -n 50"],
  },
  {
    pattern: /\bmkfs\.[a-z0-9]+\s+\/dev\/\S+/i,
    riskLevel: "critical",
    blocked: true,
    reason: "该命令会格式化磁盘或分区，可能导致数据不可恢复。",
    safeAlternatives: ["lsblk -f", "blkid"],
  },
  {
    pattern: /\bdd\s+.+\bof=\/dev\/(sd|vd|xvd|hd|nvme)\S+/i,
    riskLevel: "critical",
    blocked: true,
    reason: "该 dd 命令会直接写入块设备，可能破坏磁盘数据。",
    safeAlternatives: ["lsblk -f", "df -hT"],
  },
  {
    pattern: /\bsystemctl\s+stop\s+(ssh|sshd)\b/i,
    riskLevel: "critical",
    blocked: true,
    reason: "停止 SSH 服务可能导致当前远程连接断开并无法重新登录。",
    safeAlternatives: ["systemctl status ssh --no-pager", "systemctl status sshd --no-pager"],
  },
  {
    pattern: /\b(iptables\s+-F|ufw\s+disable)\b/i,
    riskLevel: "critical",
    blocked: true,
    reason: "清空防火墙规则或关闭防火墙可能暴露服务或切断访问策略。",
    safeAlternatives: ["iptables -S", "ufw status verbose"],
  },
  {
    pattern: /\b(shutdown|poweroff|halt)\b|\breboot\b/i,
    riskLevel: "high",
    blocked: false,
    reason: "该命令会重启或关闭系统，可能中断业务和当前连接。",
    safeAlternatives: ["uptime", "who", "systemctl list-jobs"],
    confirmText: "我确认要重启或关闭系统",
  },
  {
    pattern: /(^|[;&|]\s*)rm\s+-[^\n]*r[^\n]*f[^\n]*\s+\S*[*?]\S*/i,
    riskLevel: "high",
    blocked: false,
    reason: "该命令会递归强制删除匹配路径，可能不可恢复。",
    safeAlternatives: ["ls -lah", "find . -maxdepth 1 -print | head -n 50"],
    confirmText: "我确认要删除这些文件",
  },
  {
    pattern: /(^|[;&|]\s*)rm\s+-[^\n]*r[^\n]*f[^\n]*\s+\/[^;&|]+/i,
    riskLevel: "high",
    blocked: false,
    reason: "该命令会递归强制删除绝对路径下的内容，可能不可恢复。",
    safeAlternatives: ["ls -lah <target>", "find <target> -maxdepth 1 -print | head -n 50"],
    confirmText: "我确认要删除目标路径",
  },
  {
    pattern: /\bdocker\s+system\s+prune\b.*\s-a\b/i,
    riskLevel: "high",
    blocked: false,
    reason: "该命令会删除未使用镜像、容器、网络和缓存，可能影响回滚能力。",
    safeAlternatives: ["docker system df", "docker ps -a", "docker images"],
    confirmText: "我确认要清理 Docker 资源",
  },
  {
    pattern: /\bkubectl\s+delete\s+(namespace|ns)\b/i,
    riskLevel: "high",
    blocked: false,
    reason: "删除 Kubernetes namespace 会删除其中的大量资源。",
    safeAlternatives: ["kubectl get ns", "kubectl get all -n <namespace>"],
    confirmText: "我确认要删除 Kubernetes 命名空间",
  },
];

export function assessCommandRisk(command: string, username?: string | null): CommandRiskResponse {
  const trimmed = command.trim();
  const matched = patterns.find((item) => item.pattern.test(trimmed));
  if (matched) {
    const riskLevel =
      username === "root" && isRootSensitiveCommand(trimmed)
        ? bumpRisk(matched.riskLevel)
        : matched.riskLevel;
    return {
      riskLevel,
      blocked: matched.blocked,
      reason:
        username === "root" && riskLevel !== matched.riskLevel
          ? `${matched.reason} root 用户下风险上调。`
          : matched.reason,
      safeAlternatives: matched.safeAlternatives,
      confirmText: matched.confirmText,
    };
  }

  const baseRisk: RiskLevel = isReadOnlyCommand(trimmed) ? "low" : "medium";
  const riskLevel =
    username === "root" && isRootSensitiveCommand(trimmed) ? bumpRisk(baseRisk) : baseRisk;

  return {
    riskLevel,
    blocked: false,
    reason:
      riskLevel !== baseRisk
        ? "root 用户下执行删除、权限或服务变更命令，影响范围更大。"
        : "未发现明显高危操作。",
    safeAlternatives: [],
  };
}

function bumpRisk(level: RiskLevel): RiskLevel {
  if (level === "low") return "medium";
  if (level === "medium") return "high";
  return "critical";
}

function isRootSensitiveCommand(command: string) {
  const lower = command.toLowerCase();
  return /\b(rm|chmod|chown|systemctl)\b/.test(lower);
}

function isReadOnlyCommand(command: string) {
  const parts = command.trim().toLowerCase().split(/\s+/);
  const first = parts[0] === "sudo" ? parts[1] : parts[0];
  return [
    "ls",
    "pwd",
    "cat",
    "tail",
    "head",
    "grep",
    "find",
    "ps",
    "top",
    "free",
    "df",
    "du",
    "uptime",
    "who",
    "id",
    "uname",
    "hostname",
    "hostnamectl",
    "ip",
    "ss",
    "curl",
    "journalctl",
    "systemctl",
    "docker",
    "kubectl",
  ].includes(first);
}
