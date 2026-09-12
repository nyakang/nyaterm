import { Check, ChevronDown } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdChevronRight } from "react-icons/md";
import { ConnectionRecordingSettings } from "@/components/sessions/ConnectionRecordingSettings";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  isValidSerialBaudRate,
  MAX_SERIAL_BAUD_RATE,
  MIN_SERIAL_BAUD_RATE,
  normalizeSerialBaudRateInput,
  SERIAL_BAUD_RATE_OPTIONS,
} from "@/lib/serial";
import { cn } from "@/lib/utils";
import type { RecordingMode } from "@/types/global";

interface SerialPortOption {
  unavailable?: boolean;
  value: string;
}

interface SerialFormProps {
  serialPortName: string;
  setSerialPortName: (v: string) => void;
  serialPortOptions: SerialPortOption[];
  serialPortsLoading: boolean;
  serialPortsError: string;
  onSerialPortDropdownOpen: () => void;
  baudRate: string;
  setBaudRate: (v: string) => void;
  dataBits: string;
  setDataBits: (v: string) => void;
  parity: string;
  setParity: (v: string) => void;
  stopBits: string;
  setStopBits: (v: string) => void;
  backspaceMode: string;
  setBackspaceMode: (v: string) => void;
  modemUploadProtocol: "xmodem" | "ymodem" | "zmodem";
  setModemUploadProtocol: (v: "xmodem" | "ymodem" | "zmodem") => void;
  recordingUseGlobal: boolean;
  setRecordingUseGlobal: (v: boolean) => void;
  recordingAutoStart: boolean;
  setRecordingAutoStart: (v: boolean) => void;
  recordingMode: RecordingMode;
  setRecordingMode: (v: RecordingMode) => void;
  encoding: string;
  setEncoding: (v: string) => void;
}

function RequiredMark() {
  return <span className="ml-0.5 text-destructive">*</span>;
}

interface BaudRatePickerProps {
  value: string;
  onValueChange: (v: string) => void;
}

function BaudRatePicker({ value, onValueChange }: BaudRatePickerProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [customDraft, setCustomDraft] = useState("");
  const standardValueSet = useMemo(() => new Set(SERIAL_BAUD_RATE_OPTIONS), []);
  const isStandardValue = standardValueSet.has(value);
  const isCustomValue = !!value && !isStandardValue;
  const isDraftValid = isValidSerialBaudRate(customDraft);
  const showDraftError = customDraft.length > 0 && !isDraftValid;
  const normalizedDraft = isDraftValid ? String(Number(customDraft)) : "";

  useEffect(() => {
    if (!open) return;
    setCustomDraft(isCustomValue ? value : "");
  }, [isCustomValue, open, value]);

  const selectPreset = (nextValue: string) => {
    onValueChange(nextValue);
    setCustomDraft("");
    setOpen(false);
  };

  const commitCustom = (close = true) => {
    if (!isDraftValid) return;
    onValueChange(normalizedDraft);
    setCustomDraft(normalizedDraft);
    if (close) {
      setOpen(false);
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          className="mt-1 h-8 w-full justify-between px-3 text-xs font-normal"
        >
          <span className={cn("min-w-0 truncate", value ? "" : "text-muted-foreground")}>
            {value || t("dialog.selectBaudRate", "Select baud rate")}
          </span>
          <span className="ml-auto flex shrink-0 items-center gap-1.5">
            {isCustomValue && isValidSerialBaudRate(value) && (
              <span className="rounded bg-primary/10 px-1.5 py-0.5 text-[0.625rem] leading-none text-primary">
                {t("dialog.customBaudRate", "Custom")}
              </span>
            )}
            <ChevronDown className="size-3.5 opacity-50" />
          </span>
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="bottom"
        sideOffset={4}
        collisionPadding={16}
        className="w-[var(--radix-popover-trigger-width)] min-w-[13rem] overflow-hidden p-1.5"
      >
        <div className="grid grid-cols-2 gap-1">
          {SERIAL_BAUD_RATE_OPTIONS.map((option) => {
            const selected = value === option;
            return (
              <button
                key={option}
                type="button"
                className={cn(
                  "flex h-7 items-center justify-between rounded px-2 text-left text-xs transition-colors hover:bg-accent",
                  selected ? "bg-primary/15 text-primary" : "text-foreground",
                )}
                onClick={() => selectPreset(option)}
              >
                <span className="truncate">{option}</span>
                {selected && <Check className="size-3.5 shrink-0" />}
              </button>
            );
          })}
        </div>
        <div className="mt-1.5 border-t pt-1.5">
          <div className="mb-1 px-1 text-[0.6875rem] font-medium text-muted-foreground">
            {t("dialog.customBaudRate", "Custom")}
          </div>
          <div className="flex items-center gap-1.5">
            <Input
              className="h-7 min-w-0 flex-1 px-2 text-xs"
              inputMode="numeric"
              pattern="[0-9]*"
              placeholder={t("dialog.customBaudRatePlaceholder", "e.g. 74880")}
              value={customDraft}
              aria-invalid={showDraftError}
              onChange={(event) => setCustomDraft(normalizeSerialBaudRateInput(event.target.value))}
              onBlur={() => {
                if (isDraftValid && normalizedDraft !== value) {
                  commitCustom(false);
                }
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  commitCustom();
                } else if (event.key === "Escape") {
                  setOpen(false);
                }
              }}
            />
            <Button
              type="button"
              variant="outline"
              size="icon-xs"
              disabled={!isDraftValid}
              aria-label={t("dialog.applyCustomBaudRate", "Apply custom baud rate")}
              onClick={() => commitCustom()}
            >
              <Check className="size-3.5" />
            </Button>
          </div>
          {showDraftError && (
            <p className="mt-1 px-1 text-[0.6875rem] leading-snug text-destructive">
              {t("dialog.baudRateInvalid", {
                min: MIN_SERIAL_BAUD_RATE,
                max: MAX_SERIAL_BAUD_RATE,
                defaultValue: "Baud rate must be between {{min}} and {{max}}",
              })}
            </p>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

export function SerialForm({
  serialPortName,
  setSerialPortName,
  serialPortOptions,
  serialPortsLoading,
  serialPortsError,
  onSerialPortDropdownOpen,
  baudRate,
  setBaudRate,
  dataBits,
  setDataBits,
  parity,
  setParity,
  stopBits,
  setStopBits,
  backspaceMode,
  setBackspaceMode,
  modemUploadProtocol,
  setModemUploadProtocol,
  recordingUseGlobal,
  setRecordingUseGlobal,
  recordingAutoStart,
  setRecordingAutoStart,
  recordingMode,
  setRecordingMode,
  encoding,
  setEncoding,
}: SerialFormProps) {
  const { t } = useTranslation();
  const [advancedOpen, setAdvancedOpen] = useState(false);

  return (
    <div className="space-y-3 w-full">
      <div className="grid grid-cols-[minmax(0,1fr)_9rem] gap-3">
        <div className="min-w-0">
          <Label className="text-xs font-medium text-foreground/80">
            {t("dialog.serialPort", "Serial Port")}
            <RequiredMark />
          </Label>
          <Select
            value={serialPortName || undefined}
            onValueChange={setSerialPortName}
            onOpenChange={(open) => {
              if (open) {
                onSerialPortDropdownOpen();
              }
            }}
          >
            <SelectTrigger className="mt-1 h-8 text-xs font-normal">
              <SelectValue placeholder={t("dialog.selectSerialPort", "Select Serial Port")} />
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
                  {t("dialog.loadingSerialPorts", "Loading serial ports...")}
                </div>
              ) : serialPortOptions.length === 0 ? (
                <div className="px-2 py-1.5 text-xs text-muted-foreground">
                  {t("dialog.noSerialPortsFound", "No serial ports found")}
                </div>
              ) : (
                serialPortOptions.map((port) => (
                  <SelectItem key={port.value} value={port.value}>
                    {port.unavailable
                      ? t("dialog.serialPortUnavailable", {
                          port: port.value,
                          defaultValue: "{{port}} (Unavailable)",
                        })
                      : port.value}
                  </SelectItem>
                ))
              )}
            </SelectContent>
          </Select>
          {serialPortsError && (
            <p className="mt-1 text-[0.6875rem] text-destructive">{serialPortsError}</p>
          )}
        </div>
        <div className="min-w-0">
          <Label className="text-xs font-medium text-foreground/80">
            {t("dialog.baudRate", "Baud Rate")}
          </Label>
          <BaudRatePicker value={baudRate} onValueChange={setBaudRate} />
        </div>
      </div>
      <div className="grid grid-cols-[4.5rem_minmax(8rem,1fr)_4.5rem] gap-3">
        <div className="min-w-0">
          <Label className="text-xs font-medium text-foreground/80">
            {t("dialog.dataBits", "Data Bits")}
          </Label>
          <Select value={dataBits} onValueChange={setDataBits}>
            <SelectTrigger className="mt-1 h-8 text-xs font-normal">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="5">5</SelectItem>
              <SelectItem value="6">6</SelectItem>
              <SelectItem value="7">7</SelectItem>
              <SelectItem value="8">8</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="min-w-0">
          <Label className="text-xs font-medium text-foreground/80">
            {t("dialog.parity", "Parity")}
          </Label>
          <Select value={parity} onValueChange={setParity}>
            <SelectTrigger className="mt-1 h-8 text-xs font-normal">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">{t("dialog.parityNone", "None")}</SelectItem>
              <SelectItem value="odd">{t("dialog.parityOdd", "Odd")}</SelectItem>
              <SelectItem value="even">{t("dialog.parityEven", "Even")}</SelectItem>
              <SelectItem value="mark">{t("dialog.parityMark", "Mark")}</SelectItem>
              <SelectItem value="space">{t("dialog.paritySpace", "Space")}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="min-w-0">
          <Label className="text-xs font-medium text-foreground/80">
            {t("dialog.stopBits", "Stop Bits")}
          </Label>
          <Select value={stopBits} onValueChange={setStopBits}>
            <SelectTrigger className="mt-1 h-8 text-xs font-normal">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="1">1</SelectItem>
              <SelectItem value="1.5">1.5</SelectItem>
              <SelectItem value="2">2</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
      <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
        <CollapsibleTrigger className="group flex w-full items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground">
          <MdChevronRight
            className={`text-sm transition-transform duration-200 ${advancedOpen ? "rotate-90" : ""}`}
          />
          <span>{t("dialog.advancedConfig")}</span>
        </CollapsibleTrigger>
        <CollapsibleContent className="mt-3">
          <Tabs defaultValue="terminal" className="w-full">
            <TabsList className="grid h-8 w-full grid-cols-1 pointer-events-auto">
              <TabsTrigger value="terminal" className="text-xs">
                {t("dialog.encodingSettings")}
              </TabsTrigger>
            </TabsList>
            <TabsContent value="terminal" className="mt-3 border-0 outline-none">
              <div className="space-y-3 rounded-lg border bg-accent/25 p-3">
                <div className="grid gap-3 sm:grid-cols-2">
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.backspaceMode", "Backspace Mode")}
                    </Label>
                    <Select value={backspaceMode} onValueChange={setBackspaceMode}>
                      <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="ctrl_h">
                          {t("dialog.backspaceCtrlH", "Ctrl+H (BS)")}
                        </SelectItem>
                        <SelectItem value="del">{t("dialog.backspaceDel", "DEL (0x7F)")}</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("connection.encoding")}
                    </Label>
                    <Select value={encoding} onValueChange={setEncoding}>
                      <SelectTrigger className="mt-1 h-8 w-full text-xs">
                        <SelectValue placeholder={t("connection.encodingFollowGlobal")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="global">
                          {t("connection.encodingFollowGlobal")}
                        </SelectItem>
                        <SelectItem value="UTF-8">UTF-8</SelectItem>
                        <SelectItem value="GBK">GBK</SelectItem>
                        <SelectItem value="GB2312">GB2312</SelectItem>
                        <SelectItem value="GB18030">GB18030</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.serialModemUploadProtocol", "Modem upload protocol")}
                    </Label>
                    <Select
                      value={modemUploadProtocol}
                      onValueChange={(value) =>
                        setModemUploadProtocol(value as "xmodem" | "ymodem" | "zmodem")
                      }
                    >
                      <SelectTrigger className="mt-1 h-8 w-full text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="xmodem">XMODEM</SelectItem>
                        <SelectItem value="ymodem">YMODEM</SelectItem>
                        <SelectItem value="zmodem">ZMODEM</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                </div>
                <ConnectionRecordingSettings
                  useGlobal={recordingUseGlobal}
                  onUseGlobalChange={setRecordingUseGlobal}
                  autoStart={recordingAutoStart}
                  onAutoStartChange={setRecordingAutoStart}
                  mode={recordingMode}
                  onModeChange={setRecordingMode}
                />
              </div>
            </TabsContent>
          </Tabs>
        </CollapsibleContent>
      </Collapsible>
    </div>
  );
}
