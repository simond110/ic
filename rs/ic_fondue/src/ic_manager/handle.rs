use nix::unistd::Pid;
use rand::Rng;
use url::{Host, Url};

use crate::prod_tests::{cli::AuthorizedSshAccount, farm};
use fondue::{
    log::info,
    util::{InfStreamOf, PermOf},
};
use ic_prep_lib::prep_state_directory::IcPrepStateDir;
use ic_registry_subnet_type::SubnetType;
use ic_types::messages::{HttpStatusResponse, ReplicaHealthStatus};
use ic_types::SubnetId;
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};
use tokio::time;

/// A handle used by tests to interact with the IC.
///
/// The provided information is kept as general and simple as possible.
/// Currently, the structure only exposes the list of URLs. It also exposes the
/// path to the working directory as prepared by `ic-prep`. This can be used,
/// e.g., to read the initial registry local store.
///
/// While the IcHandle will always present a list of Urls to the test author,
/// any additional fields might change and are implementation specific. This is
/// on purpose as we do not want to overspecify this interface, as the needs of
/// test authors is likely vary across both components/teams and time. Test
/// owners can build their own abstractions on top of this Handle and also
/// change `ic-prep` if necessary.
#[derive(Clone, Debug)]
pub struct IcHandle {
    /// The list of Public API endpoints of this IC.
    pub public_api_endpoints: Vec<IcEndpoint>,
    /// The list of Public API endpoints of malicious nodes of this IC.
    pub malicious_public_api_endpoints: Vec<IcEndpoint>,
    /// Path to the working dir as prepared by `ic-prep`.
    pub ic_prep_working_dir: Option<IcPrepStateDir>,
}

#[derive(Clone, Debug)]
pub enum RuntimeDescriptor {
    Process(Pid),
    Vm(FarmInfo),
    Unknown,
}

#[derive(Clone, Debug)]
pub struct FarmInfo {
    pub url: Url,
    pub vm_name: String,
    pub group_name: String,
}

#[derive(Clone, Debug)]
pub struct IcSubnet {
    pub id: SubnetId,
    pub type_of: SubnetType,
}

#[derive(Clone, Debug)]
pub struct IcEndpoint {
    /// A descriptor of this endpoint. This is public to give us a simple
    /// way of restarting. See node_restart_test for an example.
    pub runtime_descriptor: RuntimeDescriptor,

    /// A URL pointing to an endpoint that implements the Public Spec.
    pub url: Url,
    /// A URL pointing to an endpoint hosting the metrics for the replica.
    pub metrics_url: Option<Url>,

    /// Set if `url` points to the public endpoint of the root subnet.
    ///
    /// # Note
    ///
    /// This coincides with the NNS subnet, if an NNS subnet is present. Note
    /// that the root subnet is a protocol level concept while the NNS is an
    /// application level concept.
    ///
    /// See also: https://docs.dfinity.systems/spec/public/#certification-delegation
    pub is_root_subnet: bool,

    /// The subnet that the node was initially assigned to. `None` if the
    /// respective node was unassigned when the IC was bootstrapped.
    pub subnet: Option<IcSubnet>,

    /// A timestamp when a node gets started.
    pub started_at: Instant,

    /// A list of accounts and associated SSH key pairs that were installed when
    /// the IC was boostrapped. The private and public key are the raw content
    /// of the corresponding file generated by `ssh-keygen`, as provided to the
    /// test driver.
    pub ssh_key_pairs: Vec<AuthorizedSshAccount>,
}

pub trait IcControl {
    fn start_node(&self) -> IcEndpoint;
    fn kill_node(&self);
    fn restart_node(&self) -> IcEndpoint;
    fn ip_address(&self) -> Option<IpAddr>;
    fn hostname(&self) -> Option<String>;
}

impl IcControl for IcEndpoint {
    fn kill_node(&self) {
        if let RuntimeDescriptor::Vm(info) = &self.runtime_descriptor {
            let farm = farm::Farm::new(info.url.clone());
            farm.destroy_vm(&info.group_name, &info.vm_name)
                .expect("failed to destroy VM");
        } else {
            panic!("Cannot kill a node with IcControl that is not hosted by farm.");
        }
    }

    fn restart_node(&self) -> Self {
        if let RuntimeDescriptor::Vm(info) = &self.runtime_descriptor {
            let farm = farm::Farm::new(info.url.clone());
            farm.reboot_vm(&info.group_name, &info.vm_name)
                .expect("failed to reboot VM");
            Self {
                started_at: Instant::now(),
                ..self.clone()
            }
        } else {
            panic!("Cannot restart a node with IcControl that is not hosted by farm.");
        }
    }

    fn start_node(&self) -> Self {
        if let RuntimeDescriptor::Vm(info) = &self.runtime_descriptor {
            let farm = farm::Farm::new(info.url.clone());
            farm.start_vm(&info.group_name, &info.vm_name)
                .expect("failed to destroy VM");
            Self {
                started_at: Instant::now(),
                ..self.clone()
            }
        } else {
            panic!("Cannot start a node with IcControl that is not hosted by farm.");
        }
    }

    /// An IpAddress assigned to the Virtual Machine of the corresponding node,
    /// if available.
    fn ip_address(&self) -> Option<IpAddr> {
        self.url.host().and_then(|h| match h {
            Host::Domain(_) => None,
            Host::Ipv4(ip_addr) => Some(IpAddr::V4(ip_addr)),
            Host::Ipv6(ip_addr) => Some(IpAddr::V6(ip_addr)),
        })
    }

    /// Returns the hostname assigned to the Virtual Machine of the
    /// corresponding node, if available.
    fn hostname(&self) -> Option<String> {
        self.url.host().and_then(|h| match h {
            Host::Domain(s) => Some(s.to_string()),
            Host::Ipv4(_) => None,
            Host::Ipv6(_) => None,
        })
    }
}

impl<'a> IcHandle {
    /// Returns and transfer ownership of one [IcEndpoint], removing it
    /// from the handle. If no endpoints are available it returns [None].
    pub fn take_one<R: Rng>(&mut self, rng: &mut R) -> Option<IcEndpoint> {
        if !self.public_api_endpoints.is_empty() {
            // gen_range(low, high) generates a number n s.t. low <= n < high
            Some(
                self.public_api_endpoints
                    .remove(rng.gen_range(0..self.public_api_endpoints.len())),
            )
        } else {
            None
        }
    }

    /// Returns a permutation of the available [IcEndpoint]. The [PermOf] type
    /// implements [Iterator], and hence, can be used like any other iterator.
    ///
    /// No endpoints are returned that belong to nodes that were configured with
    /// malicious behaviour!
    pub fn as_permutation<R: Rng>(&'a self, rng: &mut R) -> PermOf<'a, IcEndpoint> {
        PermOf::new(&self.public_api_endpoints, rng)
    }

    /// Returns an infinite iterator over the available [IcEndpoint]. The
    /// [InfStreamOf] type implements [Iterator], and hence, can be used
    /// like any other iterator.
    ///
    /// No endpoints are returned that belong to nodes that were configured with
    /// malicious behaviour!
    ///
    /// CAUTION: [InfStreamOf::next] never returns [None], which means calling
    /// `collect` or doing a `for i in hd.into_random_iter(rng)` will loop.
    pub fn as_random_iter<R: Rng>(&'a self, rng: &mut R) -> InfStreamOf<'a, IcEndpoint> {
        InfStreamOf::new(&self.public_api_endpoints, rng)
    }

    /// Returns and transfer ownership of one [IcEndpoint], removing it
    /// from the handle. If no endpoints are available it returns [None].
    pub fn take_one_malicious<R: Rng>(&mut self, rng: &mut R) -> Option<IcEndpoint> {
        if !self.malicious_public_api_endpoints.is_empty() {
            // gen_range(low, high) generates a number n s.t. low <= n < high
            Some(
                self.malicious_public_api_endpoints
                    .remove(rng.gen_range(0..self.malicious_public_api_endpoints.len())),
            )
        } else {
            None
        }
    }

    /// Returns a permutation of the available malicious [IcEndpoint]. The
    /// [PermOf] type implements [Iterator], and hence, can be used like any
    /// other iterator.
    ///
    /// Only endpoints are returned that belong to nodes that were configured
    /// with malicious behaviour!
    pub fn as_permutation_malicious<R: Rng>(&'a self, rng: &mut R) -> PermOf<'a, IcEndpoint> {
        PermOf::new(&self.malicious_public_api_endpoints, rng)
    }

    /// Returns an infinite iterator over the available malicious [IcEndpoint].
    /// The [InfStreamOf] type implements [Iterator], and hence, can be used
    /// like any other iterator.
    ///
    /// Only endpoints are returned that belong to nodes that were configured
    /// with malicious behaviour!
    ///
    /// CAUTION: [InfStreamOf::next] never returns [None], which means calling
    /// `collect` or doing a `for i in hd.into_random_iter(rng)` will loop.
    pub fn as_random_iter_malicious<R: Rng>(&'a self, rng: &mut R) -> InfStreamOf<'a, IcEndpoint> {
        InfStreamOf::new(&self.malicious_public_api_endpoints, rng)
    }
}

impl<'a> IcEndpoint {
    /// Returns true if [IcEndpoint] is healthy, i.e. up and running and ready
    /// for interaction. A status of the endpoint is requested from the
    /// public API.
    pub async fn healthy(&self) -> bool {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .expect("cannot build a reqwest client")
            .get(
                self.url
                    .clone()
                    .join("api/v2/status")
                    .expect("failed to join URLs"),
            )
            .send()
            .await;

        // Do not fail, as an error may be retriable.
        // Keep trying until the time limit is not exceeded.
        if let Err(ref e) = response {
            println!("Response error: {:?}", e);
            return false;
        }

        let cbor_response = serde_cbor::from_slice(
            &response
                .expect("failed to get a response")
                .bytes()
                .await
                .expect("failed to convert a response to bytes")
                .to_vec(),
        )
        .expect("response is not encoded as cbor");
        let status = serde_cbor::value::from_value::<HttpStatusResponse>(cbor_response)
            .expect("failed to deserialize a response to HttpStatusResponse");
        Some(ReplicaHealthStatus::Healthy) == status.replica_health_status
    }

    /// Returns [IcEndpoint] as soon as it's ready, panics if it didn't come up
    /// before a given deadline. Readiness is check through active polling
    /// of the public API.
    pub async fn assert_ready(&self, ctx: &fondue::pot::Context) {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            info!(
                ctx.logger,
                "Checking readiness of [{:?}]...",
                self.url.as_str()
            );
            if self.healthy().await {
                info!(ctx.logger, "Node [{:?}] is ready!", self.url.as_str());
                return;
            }

            info!(
                ctx.logger,
                "Node [{:?}] is not yet ready.",
                self.url.as_str()
            );
            if Instant::now().duration_since(self.started_at) > Duration::from_secs(90) {
                panic!("the IcEndpoint didn't come up within a time limit");
            }
            interval.tick().await;
        }
    }

    /// Returns the `SubnetId` of this [IcEndpoint] if it exists.
    pub fn subnet_id(&self) -> Option<SubnetId> {
        self.subnet.as_ref().map(|s| s.id)
    }
}

#[cfg(test)]
mod tests {
    use std::{net::IpAddr, time::Instant};

    use crate::ic_manager::{IcSubnet, RuntimeDescriptor};
    use ic_registry_subnet_type::SubnetType;
    use ic_test_utilities::types::ids::subnet_test_id;
    use url::Url;

    use super::{IcControl, IcEndpoint};
    #[test]
    fn returns_ipv4_and_ipv6_address() {
        let hostname = "some_host.com".to_string();
        let ipv6_addr: IpAddr = "2607:fb58:9005:42:5000:93ff:fe0b:5527".parse().unwrap();
        let ipv4_addr: IpAddr = "192.168.0.1".parse().unwrap();

        let handle = IcEndpoint {
            runtime_descriptor: RuntimeDescriptor::Unknown,
            url: Url::parse(&format!("http://{}:8080/", hostname)).unwrap(),
            metrics_url: None,
            is_root_subnet: false,
            subnet: Some(IcSubnet {
                id: subnet_test_id(1),
                type_of: SubnetType::Application,
            }),
            started_at: Instant::now(),
            ssh_key_pairs: vec![],
        };

        assert_eq!(handle.hostname().unwrap(), hostname);
        let handle = IcEndpoint {
            url: Url::parse(&format!("http://{}:8080/", ipv4_addr)).unwrap(),
            ..handle
        };
        assert_eq!(handle.ip_address().unwrap(), ipv4_addr);
        let handle = IcEndpoint {
            url: Url::parse(&format!("http://[{}]:8080/", ipv6_addr)).unwrap(),
            ..handle
        };
        assert_eq!(handle.ip_address().unwrap(), ipv6_addr);
    }
}