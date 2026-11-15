use crate::docker::{
    DockerComposeProject, DockerContainer, DockerImage, DockerNetwork, DockerVolume,
};

pub fn docker_container_matches(container: &DockerContainer, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            container.id.as_str(),
            container.name.as_str(),
            container.image.as_str(),
            container.status.as_str(),
            container.state.as_str(),
            container.ports.as_str(),
        ],
    )
}

pub fn docker_image_matches(image: &DockerImage, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            image.id.as_str(),
            image.repository.as_str(),
            image.tag.as_str(),
            image.size.as_str(),
            image.created_since.as_str(),
        ],
    )
}

pub fn docker_volume_matches(volume: &DockerVolume, query: &str) -> bool {
    docker_text_matches(query, [volume.driver.as_str(), volume.name.as_str()])
}

pub fn docker_network_matches(network: &DockerNetwork, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            network.id.as_str(),
            network.name.as_str(),
            network.driver.as_str(),
            network.scope.as_str(),
        ],
    )
}

pub fn docker_compose_project_matches(project: &DockerComposeProject, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            project.name.as_str(),
            project.status.as_str(),
            project.config_files.as_str(),
        ],
    )
}

fn docker_text_matches<const N: usize>(query: &str, values: [&str; N]) -> bool {
    query.is_empty()
        || values
            .iter()
            .any(|value| value.to_ascii_lowercase().contains(query))
}
