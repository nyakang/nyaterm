import { useTranslation } from "react-i18next";
import { MdAdd, MdDelete } from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { SEARCH_ICONS, QuickIconDef } from "../../icons";
import { useApp } from "../../../context/AppContext";

export function SearchTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();

  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <Label className="font-medium text-sm">
            {t("settings.customEngines", "Search Engines")}
          </Label>
          <Button
            variant="ghost"
            size="xs"
            className="text-primary"
            onClick={() => {
              const newEngines = [
                ...appSettings.search.custom_engines,
                { name: "New Engine", url_template: "https://example.com/search?q=%s" },
              ];
              updateAppSettings({ search: { ...appSettings.search, custom_engines: newEngines } });
            }}
          >
            <MdAdd className="text-[14px]" /> {t("common.add", "Add")}
          </Button>
        </div>

        <div className="border rounded-md overflow-hidden">
          {appSettings.search.custom_engines.map((engine, i) => (
            <div
              key={i}
              className="flex items-center gap-2 p-2 border-b last:border-0 hover:bg-accent transition-colors"
            >
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="w-8 h-8 rounded-md border flex items-center justify-center hover:bg-secondary transition-colors shrink-0"
                    title={t("settings.selectIcon", "Select Icon")}
                  >
                    {engine.icon && SEARCH_ICONS[engine.icon] ? (
                      (() => {
                        const Icon = SEARCH_ICONS[engine.icon].icon;
                        return <Icon className="text-base" style={{ color: SEARCH_ICONS[engine.icon].color }} />;
                      })()
                    ) : (
                      <MdAdd className="text-sm text-muted-foreground" />
                    )}
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="p-2 w-48 z-50">
                  <div className="grid grid-cols-6 gap-1 max-h-48 overflow-y-auto terminal-scroll">
                    {/* Option to clear icon */}
                    <DropdownMenuItem
                      className="p-1 cursor-pointer flex items-center justify-center hover:bg-secondary rounded text-xs text-muted-foreground"
                      onSelect={() => {
                        const newEngines = [...appSettings.search.custom_engines];
                        newEngines[i] = { ...newEngines[i], icon: undefined };
                        updateAppSettings({
                          search: { ...appSettings.search, custom_engines: newEngines },
                        });
                      }}
                      title="Clear icon"
                    >
                      ✕
                    </DropdownMenuItem>
                    {Object.entries(SEARCH_ICONS).map(([name, iconDef]) => {
                      const Icon = (iconDef as QuickIconDef).icon;
                      const color = (iconDef as QuickIconDef).color;
                      return (
                        <DropdownMenuItem
                          key={name}
                          className="p-1 cursor-pointer flex items-center justify-center hover:bg-secondary rounded"
                          onSelect={() => {
                            const newEngines = [...appSettings.search.custom_engines];
                            newEngines[i] = { ...newEngines[i], icon: name };
                            updateAppSettings({
                              search: { ...appSettings.search, custom_engines: newEngines },
                            });
                          }}
                          title={name}
                        >
                          <Icon className="text-base" style={{ color }} />
                        </DropdownMenuItem>
                      );
                    })}
                  </div>
                </DropdownMenuContent>
              </DropdownMenu>
              <Input
                placeholder="Name"
                className="w-1/3 text-sm"
                value={engine.name}
                onChange={(e) => {
                  const newEngines = [...appSettings.search.custom_engines];
                  newEngines[i] = { ...newEngines[i], name: e.target.value };
                  updateAppSettings({
                    search: { ...appSettings.search, custom_engines: newEngines },
                  });
                }}
              />
              <Input
                placeholder="URL Template (e.g. https://google.com/search?q=%s)"
                className="flex-1 text-sm"
                value={engine.url_template}
                onChange={(e) => {
                  const newEngines = [...appSettings.search.custom_engines];
                  newEngines[i] = { ...newEngines[i], url_template: e.target.value };
                  updateAppSettings({
                    search: { ...appSettings.search, custom_engines: newEngines },
                  });
                }}
              />
              <Button
                variant="ghost"
                size="icon-sm"
                className="text-destructive hover:bg-destructive/10"
                title={t("common.delete", "Delete")}
                onClick={() => {
                  const newEngines = appSettings.search.custom_engines.filter(
                    (_, idx) => idx !== i,
                  );
                  updateAppSettings({
                    search: { ...appSettings.search, custom_engines: newEngines },
                  });
                }}
              >
                <MdDelete className="text-[16px]" />
              </Button>
            </div>
          ))}
          {appSettings.search.custom_engines.length === 0 && (
            <div className="text-center py-6 text-xs text-muted-foreground">
              {t("settings.noCustomEngines", "No search engines available.")}
            </div>
          )}
        </div>
        <p className="text-xs mt-1 text-muted-foreground">
          {t(
            "settings.customEnginesDesc",
            "Use %s to represent the searched text in the URL template.",
          )}
        </p>
      </div>
    </div>
  );
}
