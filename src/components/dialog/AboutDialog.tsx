import { getName, getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import pkg from "../../../package.json";

interface AboutDialogProps {
    open: boolean;
    onClose: () => void;
}

export default function AboutDialog({ open, onClose }: AboutDialogProps) {
    const [appName, setAppName] = useState("Dragonfly");
    const [appVersion, setAppVersion] = useState("0.1.0");

    useEffect(() => {
        if (open) {
            getName().then(setAppName).catch(console.error);
            getVersion().then(setAppVersion).catch(console.error);
        }
    }, [open]);

    if (!open) return null;

    return (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
            <div
                className="rounded-lg w-[320px] shadow-2xl border flex flex-col items-center p-6 space-y-4"
                style={{ backgroundColor: "var(--df-bg-panel)", borderColor: "var(--df-border)" }}
            >
                <img src="/dragonfly.svg" alt="Dragonfly Logo" className="w-24 h-24 object-contain" />

                <div className="text-center">
                    <h2 className="text-lg font-bold" style={{ color: "var(--df-text)" }}>
                        {appName}
                    </h2>
                    <p className="text-xs" style={{ color: "var(--df-text-muted)" }}>
                        v{appVersion}
                    </p>
                </div>

                <p className="text-xs text-center px-4 leading-relaxed" style={{ color: "var(--df-text-dimmed)" }}>
                    A modern, high-performance SSH client built with Tauri and React.
                </p>

                <div className="flex gap-3 w-full pt-2">
                    <button
                        className="flex-1 py-2 text-xs font-medium rounded border transition-colors hover:bg-white/5"
                        style={{ borderColor: "var(--df-border)", color: "var(--df-text)" }}
                        onClick={() => openUrl(pkg.homepage)}
                    >
                        Website
                    </button>
                    <button
                        className="flex-1 py-2 text-xs font-medium rounded border transition-colors hover:bg-white/5"
                        style={{ borderColor: "var(--df-border)", color: "var(--df-text)" }}
                        onClick={() => openUrl(pkg.bugs.url)}
                    >
                        Issues
                    </button>
                </div>

                <div className="w-full">
                    <button
                        className="w-full py-2 text-xs font-medium rounded transition-colors hover:opacity-90"
                        style={{
                            backgroundColor: "var(--df-primary)",
                            color: "#fff"
                        }}
                        onClick={onClose}
                    >
                        Close
                    </button>
                </div>
            </div>
        </div>
    );
}
