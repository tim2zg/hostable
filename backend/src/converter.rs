
pub fn convert_dockerfile_to_distrobuilder(dockerfile: &str) -> String {
    let mut actions = Vec::new();
    let mut env_vars = Vec::new();
    let mut base_image = "alpinelinux".to_string();
    let mut release = "3.19".to_string(); // Default to a recent Alpine release

    let mut logical_lines = Vec::new();
    let mut current_line = String::new();

    for line in dockerfile.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.ends_with('\\') {
            let stripped = trimmed[..trimmed.len() - 1].trim_end();
            current_line.push_str(stripped);
            current_line.push(' ');
        } else {
            current_line.push_str(trimmed);
            logical_lines.push(current_line);
            current_line = String::new();
        }
    }

    // Push any remaining line if it ends abruptly
    if !current_line.is_empty() {
        logical_lines.push(current_line);
    }

    for line in logical_lines {
        if let Some(rest) = line.strip_prefix("FROM ") {
            let image_part = rest.split_whitespace().next().unwrap_or("alpine:latest");
            if image_part.starts_with("alpine") || image_part.contains("alpine") {
                base_image = "alpinelinux".to_string();
                if let Some((_, r)) = image_part.split_once(':') {
                    if r != "latest" {
                        release = r.to_string();
                    }
                }
            } else if image_part.starts_with("ubuntu") || image_part.contains("ubuntu") {
                 base_image = "ubuntu".to_string();
                 if let Some((_, r)) = image_part.split_once(':') {
                     release = r.to_string();
                 } else {
                     release = "jammy".to_string(); // Placeholder
                 }
            }
        } else if let Some(rest) = line.strip_prefix("RUN ") {
            // Append to actions shell script
            actions.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("ENV ") {
            // Split by space for multiple envs on one line, or just take the whole thing
            // Complex linuxserver ENV blocks have format: ENV KEY=VAL KEY2=VAL2
            // We can just dump the whole thing as a single export since they use KEY=VAL
            env_vars.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("CMD ") {
            actions.push(format!("echo '#!/bin/sh' > /etc/local.d/hostable.start"));
            // Safe escape for complex commands
            actions.push(format!("echo '{}' >> /etc/local.d/hostable.start", rest.replace("'", "'\\''")));
            actions.push("chmod +x /etc/local.d/hostable.start".to_string());
            actions.push("rc-update add local default".to_string());
        }
    }

    let mut yaml = String::new();
    yaml.push_str("image:\n");
    yaml.push_str(&format!("  distribution: {}\n", base_image));
    yaml.push_str(&format!("  release: {}\n", release));
    yaml.push_str("  architecture: x86_64\n\n");
    
    // Add default sources (simplistic example)
    yaml.push_str("source:\n");
    yaml.push_str("  downloader: alpinelinux-http\n");
    yaml.push_str("  url: http://dl-cdn.alpinelinux.org/alpine\n\n");

    if !actions.is_empty() || !env_vars.is_empty() {
        yaml.push_str("actions:\n");
        yaml.push_str("  - trigger: post-packages\n");
        yaml.push_str("    action: |-\n");
        yaml.push_str("      #!/bin/sh\n");
        yaml.push_str("      set -e\n");
        for env in env_vars {
            yaml.push_str(&format!("      export {}\n", env));
        }
        for act in actions {
            yaml.push_str(&format!("      {}\n", act));
        }
    }

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_alpine_nginx() {
        let dockerfile = "FROM alpine:3.19\nRUN apk add nginx\nENV PORT=80\nCMD [\"nginx\", \"-g\", \"daemon off;\"]";
        let yaml = convert_dockerfile_to_distrobuilder(dockerfile);
        
        assert!(yaml.contains("distribution: alpinelinux"));
        assert!(yaml.contains("release: 3.19"));
        assert!(yaml.contains("export PORT=80"));
        assert!(yaml.contains("apk add nginx"));
        assert!(yaml.contains("nginx"));
    }

    #[test]
    fn test_convert_ubuntu_base() {
        let dockerfile = "FROM ubuntu:22.04\nRUN apt-get update";
        let yaml = convert_dockerfile_to_distrobuilder(dockerfile);
        
        assert!(yaml.contains("distribution: ubuntu"));
        assert!(yaml.contains("release: 22.04"));
        assert!(yaml.contains("apt-get update"));
    }
}
