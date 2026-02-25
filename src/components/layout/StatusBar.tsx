import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdLock } from "react-icons/md";
import { useApp } from "@/context/AppContext";

/** Footer bar showing current time and manual lock button. */
export default function StatusBar() {
  const { t } = useTranslation();
  const { setIsLocked } = useApp();
  const [time, setTime] = useState(new Date());

  useEffect(() => {
    const timer = setInterval(() => setTime(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);

  const yyyy = time.getFullYear();
  const MM = String(time.getMonth() + 1).padStart(2, "0");
  const dd = String(time.getDate()).padStart(2, "0");
  const HH = String(time.getHours()).padStart(2, "0");
  const mm = String(time.getMinutes()).padStart(2, "0");
  const formattedTime = `${yyyy}/${MM}/${dd} ${HH}:${mm}`;

  return (
    <footer
      className="h-7 text-white flex items-center justify-between px-3 text-[11px] select-none shrink-0"
      style={{ backgroundColor: "var(--df-primary)" }}
    >
      <div className="flex items-center gap-4 h-full"></div>
      <div className="flex items-center gap-2 h-full">
        <div className="flex items-center gap-1 bg-black/20 px-3 h-full">
          <span className="font-bold">{formattedTime}</span>
        </div>
        <button
          onClick={() => setIsLocked(true)}
          className="flex items-center gap-1 px-2 h-full hover:bg-white/15 transition-colors cursor-pointer"
          title={t("statusBar.lock", "Lock Screen")}
        >
          <MdLock style={{ fontSize: 14 }} />
        </button>
      </div>
    </footer>
  );
}
