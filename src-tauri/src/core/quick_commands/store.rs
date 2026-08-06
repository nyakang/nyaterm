impl QuickCommandsStore {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(QuickCommandsConfig::default()),
        }
    }

    pub fn load_from_disk(&self, app: &AppHandle) -> AppResult<()> {
        let config = config::load_quick_commands(app)?;
        self.replace(config);
        Ok(())
    }

    pub fn snapshot(&self) -> QuickCommandsConfig {
        self.config.read().unwrap().clone()
    }

    pub fn save_all(&self, app: &AppHandle, config: QuickCommandsConfig) -> AppResult<()> {
        config::save_quick_commands(app, &config)?;
        self.replace(config);
        Ok(())
    }

    pub fn upsert(
        &self,
        app: &AppHandle,
        mut command: QuickCommand,
        new_category: Option<QuickCommandCategory>,
    ) -> AppResult<QuickCommandsConfig> {
        let mut config = self.snapshot();
        let now = now_millis();

        if let Some(mut category) = new_category {
            if !config.categories.iter().any(|item| item.id == category.id) {
                if category.sort_order == 0
                    && config
                        .categories
                        .iter()
                        .any(|item| item.parent_id == category.parent_id)
                {
                    category.sort_order =
                        next_category_sort_order(&config.categories, category.parent_id.as_deref());
                }
                config.categories.push(category);
            }
        }

        command.updated_at = Some(now);

        if let Some(existing) = config
            .commands
            .iter_mut()
            .find(|item| item.id == command.id)
        {
            let original_created_at = existing.created_at;
            let original_use_count = existing.use_count;
            *existing = command;
            existing.created_at = existing.created_at.or(original_created_at);
            existing.use_count = existing.use_count.or(original_use_count);
        } else {
            command.created_at = command.created_at.or(Some(now));
            config.commands.push(command);
        }

        self.save_all(app, config.clone())?;
        Ok(config)
    }

    pub fn increment_use_count(&self, app: &AppHandle, id: &str) -> AppResult<()> {
        let mut config = self.snapshot();
        if let Some(cmd) = config.commands.iter_mut().find(|c| c.id == id) {
            cmd.use_count = Some(cmd.use_count.unwrap_or(0) + 1);
            cmd.updated_at = Some(now_millis());
            self.save_all(app, config)?;
        }
        Ok(())
    }

    pub fn import_from_file(
        &self,
        app: &AppHandle,
        file_path: &str,
        source: QuickCommandsImportSource,
    ) -> AppResult<QuickCommandsImportResult> {
        let import_config = match source {
            QuickCommandsImportSource::NyatermJson => {
                let raw = std::fs::read_to_string(file_path)?;
                parse_nyaterm_import(&raw)?
            }
            QuickCommandsImportSource::WindtermQuickbar => {
                let raw = std::fs::read_to_string(file_path)?;
                parse_windterm_quickbar(&raw)?
            }
            QuickCommandsImportSource::XshellXts => parse_xshell_xts_quick_buttons(file_path)?,
        };

        if import_config.commands.is_empty() {
            return Err(AppError::Config(
                "No valid quick commands found in import file".to_string(),
            ));
        }

        let mut config = self.snapshot();
        let stats = merge_import(&mut config, import_config)?;
        let result = QuickCommandsImportResult {
            imported_commands: stats.added_commands,
            imported_categories: stats.added_categories,
            updated_commands: stats.updated_commands,
            total_commands: config.commands.len(),
            total_categories: config.categories.len(),
        };

        self.save_all(app, config)?;
        Ok(result)
    }

    fn replace(&self, config: QuickCommandsConfig) {
        *self.config.write().unwrap() = config;
    }
}

fn next_category_sort_order(
    categories: &[QuickCommandCategory],
    parent_id: Option<&str>,
) -> i32 {
    categories
        .iter()
        .filter(|category| category.parent_id.as_deref() == parent_id)
        .map(|category| category.sort_order)
        .max()
        .unwrap_or(-1)
        + 1
}
