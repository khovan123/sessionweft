from pathlib import Path

path = Path("apps/sessionweftd/src/main.rs")
text = path.read_text()
old_init = "    let metrics = Arc::new(MetricsRegistry::new());\n"
new_init = "    let metrics_registry = Arc::new(MetricsRegistry::new());\n"
if old_init in text:
    text = text.replace(old_init, new_init, 1)
elif new_init not in text:
    raise SystemExit("metrics initialization marker not found")
old_field = "        metrics,\n    };"
new_field = "        metrics: metrics_registry,\n    };"
if old_field in text:
    text = text.replace(old_field, new_field, 1)
elif new_field not in text:
    raise SystemExit("metrics AppState marker not found")
path.write_text(text)
