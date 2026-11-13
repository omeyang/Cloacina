use crate::cli::Options;
use crate::engine::Item;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

#[derive(Debug)]
enum Rule {
    Container(String),
    Label(String, String),
    Image(String),
    Volume(String),
}

impl Rule {
    fn description(&self) -> String {
        match self {
            Self::Container(name) => format!("container {name}"),
            Self::Label(key, value) => format!("container-label {key}={value}"),
            Self::Image(name) => format!("image {name}"),
            Self::Volume(name) => format!("volume {name}"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Whitelist {
    rules: Vec<Rule>,
}

#[derive(Default)]
pub struct Protection {
    pub containers: HashSet<String>,
    pub images: HashSet<String>,
    pub volumes: HashSet<String>,
}

fn image_id(id: &str) -> &str {
    id.strip_prefix("sha256:").unwrap_or(id)
}

fn matches_image(reference: &str, image: &Item) -> bool {
    let reference_id = image_id(reference);
    image.names.iter().any(|name| name == reference)
        || reference_id == image_id(&image.id)
        || (reference_id.len() >= 12
            && reference_id.bytes().all(|b| b.is_ascii_hexdigit())
            && image_id(&image.id).starts_with(reference_id))
}

impl Whitelist {
    pub fn load(options: &Options, home: &Path, environment: Option<OsString>) -> Result<Self> {
        if options.no_whitelist {
            return Ok(Self::default());
        }
        let path = options
            .whitelist
            .clone()
            .or_else(|| environment.map(Into::into))
            .unwrap_or_else(|| home.join(".config/cloacina/whitelist.conf"));
        match fs::read_to_string(&path) {
            Ok(text) => {
                let whitelist =
                    Self::parse(&text).with_context(|| format!("白名单 {}", path.display()))?;
                println!(
                    "▸ 白名单 {}（{} 条规则）",
                    path.display(),
                    whitelist.rules.len()
                );
                Ok(whitelist)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("读取白名单 {}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut rules = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<_> = line.split_whitespace().collect();
            let rule = match parts.as_slice() {
                ["container", name] => Rule::Container((*name).into()),
                ["container-label", label] => {
                    let (key, value) = label
                        .split_once('=')
                        .filter(|(key, _)| !key.is_empty())
                        .with_context(|| format!("第 {} 行需要 label key=value", index + 1))?;
                    Rule::Label(key.into(), value.into())
                }
                ["image", reference] => Rule::Image((*reference).into()),
                ["volume", name] => Rule::Volume((*name).into()),
                _ => bail!("第 {} 行无法识别：{line}", index + 1),
            };
            rules.push(rule);
        }
        Ok(Self { rules })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn needs_inspect(&self, container: &Item) -> bool {
        self.rules.iter().any(|rule| match rule {
            Rule::Container(name) => container.names.contains(name),
            Rule::Label(..) => true,
            _ => false,
        })
    }

    // Resolve the complete protection set before allowing any deletions on this engine.
    pub fn resolve(
        &self,
        containers: &[Item],
        inspections: &[(String, Value)],
        images: &[Item],
        volumes: &[Item],
    ) -> Result<Protection> {
        let mut protection = Protection::default();
        for rule in &self.rules {
            let mut matched = false;
            match rule {
                Rule::Container(name) => {
                    for container in containers.iter().filter(|c| c.names.contains(name)) {
                        protection.containers.insert(container.id.clone());
                        matched = true;
                    }
                }
                Rule::Label(key, value) => {
                    for (id, inspection) in inspections {
                        let config = inspection.get("Config").context("容器缺少 Config")?;
                        let labels = config.get("Labels").context("容器缺少 Labels")?;
                        if !labels.is_null() && !labels.is_object() {
                            bail!("容器 Labels 不是对象");
                        }
                        if labels.get(key).and_then(Value::as_str) == Some(value) {
                            protection.containers.insert(id.clone());
                            matched = true;
                        }
                    }
                }
                Rule::Image(reference) => {
                    for image in images.iter().filter(|i| matches_image(reference, i)) {
                        protection.images.insert(image_id(&image.id).into());
                        matched = true;
                    }
                }
                Rule::Volume(name) => {
                    if volumes.iter().any(|v| &v.id == name) {
                        protection.volumes.insert(name.clone());
                        matched = true;
                    }
                }
            }
            if !matched {
                println!("  白名单未匹配本机资源：{}", rule.description());
            }
        }
        for id in &protection.containers {
            let inspection = &inspections
                .iter()
                .find(|(inspected, _)| inspected == id)
                .context("白名单容器缺少检查结果")?
                .1;
            let image = inspection
                .get("Image")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .context("白名单容器缺少镜像 ID")?;
            protection.images.insert(image_id(image).into());
            let mounts = inspection
                .get("Mounts")
                .and_then(Value::as_array)
                .context("白名单容器缺少 Mounts")?;
            for mount in mounts {
                let kind = mount
                    .get("Type")
                    .and_then(Value::as_str)
                    .context("白名单容器挂载缺少 Type")?;
                if kind == "volume" {
                    let name = mount
                        .get("Name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .context("白名单容器卷缺少 Name")?;
                    protection.volumes.insert(name.into());
                }
            }
        }
        println!(
            "  白名单保留 {} 项（容器 {}、镜像 {}、卷 {}）：",
            protection.containers.len() + protection.images.len() + protection.volumes.len(),
            protection.containers.len(),
            protection.images.len(),
            protection.volumes.len()
        );
        for container in containers
            .iter()
            .filter(|c| protection.containers.contains(&c.id))
        {
            println!(
                "    保留容器 {} ({})",
                container.names.join(" "),
                container.id
            );
        }
        for image in images.iter().filter(|i| protection.keeps_image(i)) {
            println!("    保留镜像 {} ({})", image.names.join(" "), image.id);
        }
        for volume in volumes
            .iter()
            .filter(|v| protection.volumes.contains(&v.id))
        {
            println!("    保留卷 {}", volume.id);
        }
        Ok(protection)
    }
}

impl Protection {
    pub fn keeps_image(&self, image: &Item) -> bool {
        self.images.contains(image_id(&image.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    fn item(id: &str, names: &[&str]) -> Item {
        Item {
            id: id.into(),
            names: names.iter().map(|name| (*name).into()).collect(),
            detail: String::new(),
        }
    }

    #[test]
    fn parses_comments_crlf_and_exact_label_values() {
        let whitelist = Whitelist::parse(
            "  # comment\r\n\r\n container web \r\ncontainer-label unit=web.service\n\
             container-label token=a=b\ncontainer-label empty=\nimage localhost/web:latest\nvolume data\n",
        ).unwrap();
        assert_eq!(whitelist.rules.len(), 6);
        assert!(
            matches!(&whitelist.rules[2], Rule::Label(key, value) if key == "token" && value == "a=b")
        );
        assert!(Whitelist::parse("# only comments\n\n").unwrap().is_empty());
        for invalid in [
            "unknown name",
            "container",
            "image name extra",
            "container-label unit",
            "container-label =value",
        ] {
            let error = Whitelist::parse(&format!("# comment\n{invalid}")).unwrap_err();
            assert!(error.to_string().contains("第 2 行"), "{error}");
        }
    }

    #[test]
    fn loading_obeys_cli_environment_default_and_disable_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let default = home.join(".config/cloacina/whitelist.conf");
        fs::create_dir_all(default.parent().unwrap()).unwrap();
        let options = Options::parse_from(["cloacina"]);
        assert!(Whitelist::load(&options, home, None).unwrap().is_empty());
        fs::write(default, "container default").unwrap();
        assert_eq!(
            Whitelist::load(&options, home, None).unwrap().rules[0].description(),
            "container default"
        );
        let environment = home.join("env.conf");
        fs::write(&environment, "container environment").unwrap();
        assert_eq!(
            Whitelist::load(&options, home, Some(environment.clone().into()))
                .unwrap()
                .rules[0]
                .description(),
            "container environment"
        );
        let cli = home.join("cli.conf");
        fs::write(&cli, "container cli").unwrap();
        let cli_options = Options::parse_from(["cloacina", "--whitelist", cli.to_str().unwrap()]);
        assert_eq!(
            Whitelist::load(&cli_options, home, Some(environment.into()))
                .unwrap()
                .rules[0]
                .description(),
            "container cli"
        );
        fs::write(&cli, "typo invalid").unwrap();
        assert!(Whitelist::load(&cli_options, home, None).is_err());
        let disabled = Options::parse_from(["cloacina", "--no-whitelist"]);
        assert!(
            Whitelist::load(&disabled, home, Some(cli.into()))
                .unwrap()
                .is_empty()
        );
        // An existing but unreadable-as-text path must not silently disable protection.
        assert!(Whitelist::load(&options, home, Some(home.into())).is_err());
        assert!(
            Whitelist::load(&options, home, Some(home.join("missing").into()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn names_labels_dependencies_and_duplicate_rules_protect_each_resource_once() {
        let whitelist = Whitelist::parse(
            "container web\ncontainer web\ncontainer-label unit=worker.service\n\
             image base:keep\nvolume spare\ncontainer absent\nimage missing:tag\nvolume missing",
        )
        .unwrap();
        let containers = [
            item("aaa", &["web"]),
            item("bbb", &["worker"]),
            item("ccc", &["web-extra"]),
        ];
        let inspections = [
            (
                "aaa".into(),
                json!({"Image": "sha256:111", "Config": {"Labels": null},
                "Mounts": [{"Type": "volume", "Name": "data"}, {"Type": "bind", "Source": "/data"}]}),
            ),
            (
                "bbb".into(),
                json!({"Image": "222", "Config": {"Labels": {"unit": "worker.service"}},
                "Mounts": [{"Type": "volume", "Name": "data"}]}),
            ),
            (
                "ccc".into(),
                json!({"Image": "333", "Config": {"Labels": {"unit": "worker.service.extra"}}, "Mounts": []}),
            ),
        ];
        let images = [
            item("111", &["web:latest"]),
            item("sha256:222", &["worker:latest"]),
            item("333", &["base:keep", "base:other"]),
            item("444", &["base:keep-extra"]),
        ];
        let volumes = [
            item("data", &["data"]),
            item("spare", &["spare"]),
            item("spare-extra", &["spare-extra"]),
        ];
        let protection = whitelist
            .resolve(&containers, &inspections, &images, &volumes)
            .unwrap();
        assert_eq!(
            protection.containers,
            HashSet::from(["aaa".into(), "bbb".into()])
        );
        assert_eq!(
            protection.images,
            HashSet::from(["111".into(), "222".into(), "333".into()])
        );
        assert_eq!(
            protection.volumes,
            HashSet::from(["data".into(), "spare".into()])
        );
        assert!(protection.keeps_image(&images[1]));
        assert!(!protection.keeps_image(&images[3]));
    }

    #[test]
    fn image_rules_accept_tags_full_ids_and_at_least_twelve_digit_prefixes() {
        let image = item("sha256:abcdef1234567890", &["web:one", "web:two"]);
        for reference in [
            "web:one",
            "web:two",
            "abcdef1234567890",
            "sha256:abcdef1234567890",
            "abcdef123456",
            "sha256:abcdef123456",
        ] {
            assert!(matches_image(reference, &image), "{reference}");
        }
        for reference in ["web", "web:*", "web:three", "abcdef12345", "bcdef1234567"] {
            assert!(!matches_image(reference, &image), "{reference}");
        }
    }

    #[test]
    fn malformed_inspections_cannot_be_used_to_allow_cleanup() {
        let named = Whitelist::parse("container web").unwrap();
        let labelled = Whitelist::parse("container-label unit=web.service").unwrap();
        let containers = [item("aaa", &["web"])];
        assert!(named.resolve(&containers, &[], &[], &[]).is_err());
        for inspection in [
            json!({}),
            json!({"Image": "111"}),
            json!({"Image": "111", "Mounts": [{"Type": "volume"}]}),
        ] {
            assert!(
                named
                    .resolve(&containers, &[("aaa".into(), inspection)], &[], &[])
                    .is_err()
            );
        }
        for inspection in [json!({}), json!({"Config": {"Labels": []}})] {
            assert!(
                labelled
                    .resolve(&containers, &[("aaa".into(), inspection)], &[], &[])
                    .is_err()
            );
        }
    }
}
