# boxxy-apps Agents & Architecture

## Responsibilities
This crate provides the "Boxxy Apps" container, a specialized tab for Lua-scripted utility applications within the terminal environment. It includes an AI-powered app generator, a Lua execution engine with GTK4 widget bindings, and a sidebar for managing installed apps.

## Public Components

### `BoxxyAppsComponent`
A GTK4 component that serves as the entry point/container for boxxy-apps. Displays a sidebar with installed apps and a content area for running them.

**Inputs (`BoxxyAppsInput`):**
- `RunScript(String)`: Executes a Lua script and displays the resulting widget.
- `OpenCreateDialog`: Opens the AI-powered app creation dialog (clears content area first).
- `SetClient(Option<Arc<dyn LlmClient>>)`: Sets the AI client for code generation.
- `LoadApps`: Scans the apps config directory and populates the sidebar list.
- `RunAppFile(PathBuf)`: Loads and executes a `.lua` file.
- `OpenExternal(PathBuf)`: Opens a `.lua` file in the user's default text editor.
- `RemoveApp(PathBuf)`: Deletes a `.lua` file and refreshes the list.
- `ReorderApp(PathBuf, PathBuf)`: Reorders apps via drag-and-drop; persists order to `order.json`.

**Usage:**
- Designed to be a singleton tab within the main window (enforced by `boxxy-window`).

### `CreateAppDialog`
A GTK4 component providing an `libadwaita::Window` dialog for AI-assisted app creation.

**Inputs (`CreateAppInput`):**
- `SubmitPrompt`: Sends the user's description to the LLM to generate Lua code.
- `ReceiveCode(String)`: Receives generated code, renders a preview.
- `SaveApp`: Saves the generated code as a `.lua` file.
- `SetClient(Option<Arc<dyn LlmClient>>)`: Sets the AI client.

**Outputs (`CreateAppOutput`):**
- `AppSaved`: Emitted after successful save; triggers `LoadApps` in the parent.

**Dialog behavior:**
- Uses `set_hide_on_close: true` to avoid destruction on close (safe to re-present).
- Save button is a pill button at the bottom of the content area.

### `BoxxyAppEngine`
The Lua execution engine that bridges Lua scripts to GTK4 widgets.

**Lua API exposed to scripts:**
- `boxxy.ui.label({ text=str })` -> Widget
- `boxxy.ui.button({ label=str, on_click=fn })` -> Widget (callbacks execute via `Lua::clone()` + `RegistryKey`)
- `boxxy.ui.entry({ placeholder=str })` -> Widget
- `boxxy.ui.box({ orientation, spacing, children })` -> Widget
- `boxxy.utils.run_command(cmd, {args})` -> (success, stdout, stderr)
- `boxxy.utils.notify(msg)`

**Widget methods (colon syntax in Lua):**
- `widget:get_text()` -> reads text from Entry or Label
- `widget:set_text(str)` -> updates text on Entry or Label

## App Row Features
Each app in the sidebar list has:
- Click to run the app
- Edit button (opens in default text editor via `gio::AppInfo`)
- Delete button (removes the `.lua` file)
- Drag-and-drop reordering (persisted to `order.json`)

## Sidebar Footer
A non-interactive warning row is shown at the bottom of the sidebar (below the app list ScrolledWindow):
- Warning icon (`dialog-warning-symbolic`, 14px, 50% opacity)
- Label: "Experimental · Review code before run" (`.caption` CSS class, 50% opacity, wrapping)

This serves as a security reminder since Lua apps execute arbitrary code.

## File Storage
- Apps stored as `.lua` files in `~/.config/boxxy-terminal/apps/`
- Order persisted in `~/.config/boxxy-terminal/apps/order.json`

## Dependencies
- `mlua` (Lua 5.4, vendored) for script execution
- `boxxy-ai-core` for `LlmClient` trait
- `serde_json` for order persistence
- `directories` for config paths
