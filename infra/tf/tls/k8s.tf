resource "kubernetes_namespace" "infra" {
	for_each = toset(["traefik", "imagor"])

	metadata {
		name = each.key
	}
}

# Must be created in every namespace it is used in
resource "kubernetes_secret" "ingress_tls_cert" {
	for_each = toset([
		for x in kubernetes_namespace.infra:
		x.metadata.0.name
	])

	metadata {
		name = "ingress-tls-cert"
		namespace = each.value
	}

	type = "kubernetes.io/tls"

	data = {
		"tls.crt" = cloudflare_origin_ca_certificate.rivet_gg.certificate
		"tls.key" = tls_private_key.cf_origin_rivet_gg.private_key_pem
	}
}

resource "kubernetes_secret" "ingress_tls_ca_cert" {
	metadata {
		name = "ingress-tls-ca-cert"
		namespace = kubernetes_namespace.infra["traefik"].metadata.0.name
	}

	data = {
		"tls.ca" = local.cloudflare_ca_cert
	}
}
