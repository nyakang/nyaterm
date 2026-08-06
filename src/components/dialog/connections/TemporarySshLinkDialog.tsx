import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { TiFlashOutline } from "react-icons/ti";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { invoke } from "@/lib/invoke";
import { isValidSerialBaudRate, normalizeSerialBaudRateInput } from "@/lib/serial";
import {
  createTemporarySerialLinkConfig,
  parseTemporaryLink,
  type TemporaryLinkConfig,
  type TemporaryLinkProtocol,
} from "@/lib/temporaryLink";

interface TemporarySshLinkDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConnect: (config: TemporaryLinkConfig) => void | Promise<void>;
}

export default function TemporarySshLinkDialog({
  open,
  onOpenChange,
  onConnect,
}: TemporarySshLinkDialogProps) {
  const { t } = useTranslation();
  const [protocol, setProtocol] = useState<TemporaryLinkProtocol>("ssh");
  const [value, setValue] = useState("");
  const [serialPortName, setSerialPortName] = useState("");
  const [baudRate, setBaudRate] = useState("115200");
  const [serialPorts, setSerialPorts] = useState<string[]>([]);
  const [serialPortsLoading, setSerialPortsLoading] = useState(false);
  const [serialPortsError, setSerialPortsError] = useState("");
  const [errorKey, setErrorKey] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  const parsed = useMemo(() => {
    if (protocol === "serial") return null;
    return parseTemporaryLink(protocol, value);
  }, [protocol, value]);
  const canConnect =
    protocol === "serial"
      ? Boolean(serialPortName) && isValidSerialBaudRate(baudRate)
      : value.trim().length > 0 && Boolean(parsed?.ok);

  const loadSerialPorts = useCallback(async () => {
    setSerialPortsLoading(true);
    setSerialPortsError("");

    try {
      const ports = await invoke<string[]>("list_serial_ports");
      setSerialPorts(ports);
      setSerialPortName((current) => (current && !ports.includes(current) ? "" : current));
    } catch (error) {
      setSerialPortsError(String(error));
    } finally {
      setSerialPortsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!open) {
      setProtocol("ssh");
      setValue("");
      setSerialPortName("");
      setBaudRate("115200");
      setSerialPorts([]);
      setSerialPortsLoading(false);
      setSerialPortsError("");
      setErrorKey(null);
      return;
    }

    if (protocol === "serial") {
      void loadSerialPorts();
      return;
    }

    window.setTimeout(() => inputRef.current?.focus(), 0);
  }, [loadSerialPorts, open, protocol]);

  useEffect(() => {
    setErrorKey(null);
    if (open && protocol !== "serial") {
      window.setTimeout(() => inputRef.current?.focus(), 0);
    }
  }, [open, protocol]);

  const handleConnect = async () => {
    if (protocol === "serial") {
      if (!serialPortName) {
        setErrorKey("temporarySsh.serialPortRequired");
        return;
      }
      if (!isValidSerialBaudRate(baudRate)) {
        setErrorKey("temporarySsh.invalidBaudRate");
        return;
      }

      setErrorKey(null);
      onOpenChange(false);
      await onConnect(createTemporarySerialLinkConfig(serialPortName, Number(baudRate)));
      return;
    }

    const result = parseTemporaryLink(protocol, value);
    if (!result.ok) {
      setErrorKey(result.errorKey);
      return;
    }

    setErrorKey(null);
    onOpenChange(false);
    await onConnect(result.config);
  };

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[30rem]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-base">
            <TiFlashOutline className="text-[1rem] text-[var(--df-primary)]" />
            {t("temporarySsh.title")}
          </DialogTitle>
          <DialogDescription>{t("temporarySsh.description")}</DialogDescription>
        </DialogHeader>

        <form
          className="space-y-2"
          onSubmit={(event) => {
            event.preventDefault();
            void handleConnect();
          }}
        >
          <div className="grid grid-cols-[8rem_minmax(0,1fr)] gap-2">
            <Select
              value={protocol}
              onValueChange={(nextProtocol) => {
                setProtocol(nextProtocol as TemporaryLinkProtocol);
                setErrorKey(null);
              }}
            >
              <SelectTrigger className="h-9 w-full text-sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="ssh">{t("temporarySsh.protocolSsh")}</SelectItem>
                <SelectItem value="telnet">{t("temporarySsh.protocolTelnet")}</SelectItem>
                <SelectItem value="serial">{t("temporarySsh.protocolSerial")}</SelectItem>
              </SelectContent>
            </Select>

            {protocol === "serial" ? (
              <Select
                value={serialPortName || undefined}
                onValueChange={(portName) => {
                  setSerialPortName(portName);
                  setErrorKey(null);
                }}
                onOpenChange={(isOpen) => {
                  if (isOpen) void loadSerialPorts();
                }}
              >
                <SelectTrigger className="h-9 w-full text-sm" aria-invalid={Boolean(errorKey)}>
                  <SelectValue placeholder={t("temporarySsh.serialPortPlaceholder")} />
                </SelectTrigger>
                <SelectContent
                  position="popper"
                  align="start"
                  side="bottom"
                  sideOffset={4}
                  className="w-[var(--radix-select-trigger-width)]"
                >
                  {serialPortsLoading ? (
                    <div className="px-2 py-1.5 text-xs text-muted-foreground">
                      {t("temporarySsh.loadingSerialPorts")}
                    </div>
                  ) : serialPorts.length === 0 ? (
                    <div className="px-2 py-1.5 text-xs text-muted-foreground">
                      {t("temporarySsh.noSerialPortsFound")}
                    </div>
                  ) : (
                    serialPorts.map((portName) => (
                      <SelectItem key={portName} value={portName}>
                        {portName}
                      </SelectItem>
                    ))
                  )}
                </SelectContent>
              </Select>
            ) : (
              <Input
                ref={inputRef}
                value={value}
                onChange={(event) => {
                  setValue(event.target.value);
                  setErrorKey(null);
                }}
                placeholder={
                  protocol === "telnet"
                    ? t("temporarySsh.telnetPlaceholder")
                    : t("temporarySsh.placeholder")
                }
                aria-invalid={Boolean(errorKey)}
                className="font-mono text-sm"
              />
            )}
          </div>

          {protocol === "serial" ? (
            <Input
              value={baudRate}
              onChange={(event) => {
                setBaudRate(normalizeSerialBaudRateInput(event.target.value));
                setErrorKey(null);
              }}
              placeholder={t("temporarySsh.baudRatePlaceholder")}
              inputMode="numeric"
              pattern="[0-9]*"
              aria-invalid={Boolean(errorKey)}
              className="font-mono text-sm"
            />
          ) : null}

          {errorKey ? (
            <p className="text-xs text-destructive" role="alert">
              {t(errorKey)}
            </p>
          ) : null}
          {serialPortsError ? (
            <p className="text-xs text-destructive" role="alert">
              {t("temporarySsh.serialPortsLoadFailed", { error: serialPortsError })}
            </p>
          ) : null}
        </form>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button type="button" disabled={!canConnect} onClick={() => void handleConnect()}>
            {t("temporarySsh.connect")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
