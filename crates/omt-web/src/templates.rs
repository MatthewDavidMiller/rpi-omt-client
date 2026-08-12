use minijinja::Environment;

pub fn environment() -> Result<Environment<'static>, String> {
    let mut environment = Environment::new();
    for (name, source) in [
        ("base.html", include_str!("../templates/base.html")),
        ("about.html", include_str!("../templates/about.html")),
        (
            "dashboard.html",
            include_str!("../templates/dashboard.html"),
        ),
        (
            "diagnostics.html",
            include_str!("../templates/diagnostics.html"),
        ),
        ("error.html", include_str!("../templates/error.html")),
        ("login.html", include_str!("../templates/login.html")),
        ("network.html", include_str!("../templates/network.html")),
        (
            "reboot_confirm.html",
            include_str!("../templates/reboot_confirm.html"),
        ),
        (
            "reboot_scheduled.html",
            include_str!("../templates/reboot_scheduled.html"),
        ),
        ("system.html", include_str!("../templates/system.html")),
    ] {
        environment
            .add_template(name, source)
            .map_err(|error| error.to_string())?;
    }
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::context;

    #[test]
    fn html_templates_escape_untrusted_values() {
        let mut templates = environment().unwrap_or_else(|error| panic!("{error}"));
        templates
            .add_template("escape.html", "{{ value }}")
            .unwrap_or_else(|error| panic!("{error}"));
        let rendered = templates
            .get_template("escape.html")
            .and_then(|template| template.render(context!(value => "<script>alert(1)</script>")))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(rendered, "&lt;script&gt;alert(1)&lt;&#x2f;script&gt;");
    }
}
