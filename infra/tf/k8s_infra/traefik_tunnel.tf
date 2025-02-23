locals {
	# Specify what services to expose via the tunnel server
	tunnel_services = {
		"nomad" = {
			service = "nomad-server"
			service_namespace = kubernetes_namespace.nomad.metadata[0].name
			service_port = 4647
		}
		"api-route" = {
			service = "rivet-api-route"
			service_namespace = kubernetes_namespace.rivet_service.metadata[0].name
			service_port = 80
		}
		"vector" = {
			service = "vector"
			service_namespace = kubernetes_namespace.vector.metadata[0].name
			service_port = 6000
		}
	}

	service_traefik_tunnel = lookup(var.services, "traefik-tunnel", {
		count = var.deploy_method_cluster ? 2 : 1
		resources = {
			cpu = 500
			memory = 512
		}
	})
}

resource "kubernetes_namespace" "traefik_tunnel" {
	metadata {
		name = "traefik-tunnel"
	}
}

resource "kubernetes_priority_class" "traefik_tunnel_priority" {
	metadata {
		name = "traefik-tunnel-priority"
	}
	value = 40
}

resource "helm_release" "traefik_tunnel" {
	depends_on = [null_resource.daemons]

	name = "traefik-tunnel"
	namespace = kubernetes_namespace.traefik_tunnel.metadata.0.name
	repository = "https://traefik.github.io/charts"
	chart = "traefik"
	version = "24.0.0"
	values = [yamlencode({
		# Use Traefik v3 beta for TLS servers transport support
		image = {
			tag = "v3.0.0-beta3"
		}
		ports = {
			# Disable default ports
			web = {
				expose = false
			},
			websecure = {
				expose = false
			},

			# Expose tunnel
			tunnel = {
				port = 5000
				expose = true
				exposedPort = 5000
				protocol = "TCP"
				tls = {
					enabled = true
					options = "ingress-tunnel"
				}
			}
		}

		priorityClassName = kubernetes_priority_class.traefik_tunnel_priority.metadata.0.name

		tlsOptions = {
			"ingress-tunnel" = {
				curvePreferences = [ "CurveP384" ]

				clientAuth = {
					secretNames = [ "ingress-tls-ca-cert-locally-signed" ]
					clientAuthType = "RequireAndVerifyClientCert"
				}
			}
		}

		# Allows referencing services outside of the traefik namespace
		# TODO eventually just specify the namespace(s) that are relevant so that not pulling in configs unncessarily
		providers = {
			kubernetesCRD = {
				allowCrossNamespace = true
				labelSelector = "traefik-instance=tunnel"
			}
		}

		commonLabels = {
			"traefik-instance" = "tunnel"
		}

		resources = var.limit_resources ? {
			limits = {
				memory = "${local.service_traefik.resources.memory}Mi"
				cpu = "${local.service_traefik.resources.cpu}m"
			}
		} : null

		logs = {
			general = {
				level = "DEBUG"
			}
			access = {
				enabled = true
			}
		}

		metrics = {
			prometheus = {
				addEntryPointsLabels = false
				addRoutersLabels = true
				addServicesLabels = true
				# See lib/chirp/metrics/src/buckets.rs
				buckets = "0.001,0.0025,0.005,0.01,0.025,0.05,0.1,0.25,0.5,1.0,2.5,5.0,10.0,25.0,50.0,100.0"
			}
		}
	})]
}

resource "kubernetes_service" "traefik_tunnel_headless" {
	depends_on = [helm_release.traefik_tunnel]

	metadata {
		name = "traefik-headless"
		namespace = kubernetes_namespace.traefik_tunnel.metadata.0.name
		labels = {
			"app.kubernetes.io/name" = "traefik-headless"
		}
	}

	spec {
		selector = {
			"app.kubernetes.io/name" = "traefik"
		}

		cluster_ip = "None"

		port {
			name = "tunnel"
			port = 5000
			target_port = "tunel"
		}

		port {
			name = "metrics"
			port = 9100
			target_port = "metrics"
		}
	}
}

resource "kubectl_manifest" "traefik_tunnel_service_monitor" {
	depends_on = [helm_release.traefik_tunnel]

	yaml_body = yamlencode({
		apiVersion = "monitoring.coreos.com/v1"
		kind = "ServiceMonitor"

		metadata = {
			name = "traefik-service-monitor"
			namespace = kubernetes_namespace.traefik_tunnel.metadata.0.name
		}

		spec = {
			selector = {
				matchLabels = {
					"app.kubernetes.io/name": "traefik-headless"
				}
			}
			endpoints = [
				{
					port = "metrics"
					path = "/metrics"
				}
			]
		}
	})
}

data "kubernetes_service" "traefik_tunnel" {
	depends_on = [helm_release.traefik_tunnel]

	metadata {
		name = "traefik-tunnel"
		namespace = kubernetes_namespace.traefik_tunnel.metadata.0.name
	}
}

resource "kubectl_manifest" "traefik_nomad_router" {
	depends_on = [helm_release.traefik_tunnel]

	for_each = local.tunnel_services

	yaml_body = yamlencode({
		apiVersion = "traefik.io/v1alpha1"
		kind = "IngressRouteTCP"

		metadata = {
			name = each.key
			namespace = each.value.service_namespace
			labels = {
				"traefik-instance" = "tunnel"
			}
		}

		spec = {
			entryPoints = ["tunnel"]

			routes = [
				{
					kind = "Rule"
					match = "HostSNI(`${each.key}.tunnel.rivet.gg`)"
					services = [
						{
							name = each.value.service
							port = each.value.service_port
							# Directly access the service
							nativeLB = true
						}
					]
				}
			]

			tls = {
				secretName = "ingress-tls-cert-tunnel-server"
				options = {
					name = "ingress-tunnel",
					namespace = "traefik-tunnel"
				}

			}
		}
	})
}

