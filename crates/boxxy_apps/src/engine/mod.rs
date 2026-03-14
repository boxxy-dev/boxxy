use gtk4::prelude::*;
use mlua::{Lua, Result as LuaResult, FromLua, Value, AnyUserData};
use std::fmt;

#[derive(Clone)]
pub struct LuaWidget(pub gtk4::Widget);

impl mlua::UserData for LuaWidget {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get_text", |_, this, ()| {
            if let Some(entry) = this.0.downcast_ref::<gtk4::Entry>() {
                Ok(Some(entry.text().to_string()))
            } else if let Some(label) = this.0.downcast_ref::<gtk4::Label>() {
                Ok(Some(label.text().to_string()))
            } else {
                Ok(None)
            }
        });

        methods.add_method("set_text", |_, this, text: String| {
            if let Some(label) = this.0.downcast_ref::<gtk4::Label>() {
                label.set_text(&text);
            } else if let Some(entry) = this.0.downcast_ref::<gtk4::Entry>() {
                entry.set_text(&text);
            }
            Ok(())
        });
    }
}

impl FromLua for LuaWidget {
    fn from_lua(lua_value: Value, _lua: &Lua) -> LuaResult<Self> {
        if let Value::UserData(ud) = lua_value {
            let widget = ud.borrow::<LuaWidget>()?;
            return Ok(widget.clone());
        }
        Err(mlua::Error::FromLuaConversionError {
            from: "value",
            to: "LuaWidget".to_string(),
            message: Some("expected UserData".to_string()),
        })
    }
}

fn apply_common_props(widget: &gtk4::Widget, table: &mlua::Table) {
    if let Ok(classes) = table.get::<Vec<String>>("css_classes") {
        for class in classes {
            widget.add_css_class(&class);
        }
    }
    if let Ok(margin) = table.get::<i32>("margin_all") {
        widget.set_margin_top(margin);
        widget.set_margin_bottom(margin);
        widget.set_margin_start(margin);
        widget.set_margin_end(margin);
    }
    if let Ok(halign) = table.get::<String>("halign") {
        match halign.as_str() {
            "start" => widget.set_halign(gtk4::Align::Start),
            "end" => widget.set_halign(gtk4::Align::End),
            "center" => widget.set_halign(gtk4::Align::Center),
            "fill" => widget.set_halign(gtk4::Align::Fill),
            _ => {}
        }
    }
    if let Ok(valign) = table.get::<String>("valign") {
        match valign.as_str() {
            "start" => widget.set_valign(gtk4::Align::Start),
            "end" => widget.set_valign(gtk4::Align::End),
            "center" => widget.set_valign(gtk4::Align::Center),
            "fill" => widget.set_valign(gtk4::Align::Fill),
            _ => {}
        }
    }
}

pub struct BoxxyAppEngine {
    lua: Lua,
}

impl fmt::Debug for BoxxyAppEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BoxxyAppEngine")
    }
}

impl Default for BoxxyAppEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl BoxxyAppEngine {
    pub fn new() -> Self {
        let lua = Lua::new();
        Self { lua }
    }

    pub fn setup_api(&self) -> LuaResult<()> {
        let globals = self.lua.globals();

        let boxxy = self.lua.create_table()?;
        let ui = self.lua.create_table()?;

        // boxxy.ui.label({ text = "..." })
        ui.set("label", self.lua.create_function(|_, table: mlua::Table| {
            let text: String = table.get("text").unwrap_or_default();
            let label = gtk4::Label::builder()
                .label(&text)
                .build();
            apply_common_props(label.upcast_ref(), &table);
            Ok(LuaWidget(label.upcast()))
        })?)?;

        // boxxy.ui.button({ label = "...", on_click = fn })
        ui.set("button", self.lua.create_function(|lua, table: mlua::Table| {
            let label_text: String = table.get("label").unwrap_or_default();
            let on_click: Option<mlua::Function> = table.get("on_click").ok();

            let button = gtk4::Button::builder()
                .label(&label_text)
                .build();

            if let Some(callback) = on_click {
                let lua_clone = lua.clone();
                let registry_key = lua.create_registry_value(callback)?;

                button.connect_clicked(move |_| {
                    if let Ok(func) = lua_clone.registry_value::<mlua::Function>(&registry_key)
                        && let Err(e) = func.call::<()>(()) {
                            log::error!("Lua callback error: {}", e);
                        }
                });
            }

            apply_common_props(button.upcast_ref(), &table);
            Ok(LuaWidget(button.upcast()))
        })?)?;

        // boxxy.ui.entry({ placeholder = "..." })
        ui.set("entry", self.lua.create_function(|_, table: mlua::Table| {
            let placeholder: String = table.get("placeholder").unwrap_or_default();
            let entry = gtk4::Entry::builder()
                .placeholder_text(&placeholder)
                .build();
            apply_common_props(entry.upcast_ref(), &table);
            Ok(LuaWidget(entry.upcast()))
        })?)?;

        // boxxy.ui.box({ orientation = "vertical", spacing = 10, children = { ... } })
        ui.set("box", self.lua.create_function(|_, table: mlua::Table| {
             let orientation_str: String = table.get("orientation").unwrap_or_else(|_| "vertical".to_string());
             let spacing: i32 = table.get("spacing").unwrap_or(0);
             let children: Option<mlua::Table> = table.get("children").ok();

             let orientation = match orientation_str.as_str() {
                 "horizontal" => gtk4::Orientation::Horizontal,
                 _ => gtk4::Orientation::Vertical,
             };

             let box_widget = gtk4::Box::builder()
                 .orientation(orientation)
                 .spacing(spacing)
                 .build();

             if let Some(children_table) = children {
                 for pair in children_table.pairs::<mlua::Value, AnyUserData>() {
                     if let Ok((_, ud)) = pair
                        && let Ok(widget) = ud.borrow::<LuaWidget>() {
                             box_widget.append(&widget.0);
                        }
                 }
             }

             apply_common_props(box_widget.upcast_ref(), &table);
             Ok(LuaWidget(box_widget.upcast()))
        })?)?;

        boxxy.set("ui", ui)?;

        // boxxy.utils
        let utils = self.lua.create_table()?;

        // boxxy.utils.run_command("ls", {"-l", "-a"})
        utils.set("run_command", self.lua.create_function(|_, (cmd, args): (String, Vec<String>)| {
             use std::process::Command;

             let output = Command::new(cmd)
                 .args(args)
                 .output();

             match output {
                 Ok(o) => {
                     let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                     let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                     Ok((o.status.success(), stdout, stderr))
                 },
                 Err(e) => Ok((false, String::new(), e.to_string()))
             }
        })?)?;

        // boxxy.utils.notify("Message")
        utils.set("notify", self.lua.create_function(|_, msg: String| {
            log::info!("Notification: {}", msg);
            Ok(())
        })?)?;

        boxxy.set("utils", utils)?;
        globals.set("boxxy", boxxy)?;

        Ok(())
    }

    pub fn run_script(&self, script: &str) -> LuaResult<gtk4::Widget> {
        self.setup_api()?;
        let result: LuaWidget = self.lua.load(script).eval()?;
        Ok(result.0)
    }
}
