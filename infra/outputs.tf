output "api_url" { value = "https://${var.domain_name}" }
output "api_ecr_repository_url" { value = aws_ecr_repository.api.repository_url }
output "indexer_ecr_repository_url" { value = aws_ecr_repository.indexer.repository_url }
output "kms_key_arn" { value = aws_kms_key.signer.arn }
output "kms_public_key_note" { value = "AWS KMS exposes the secp256k1 public key via GetPublicKey, not an Ethereum address; derive and independently verify the address before funding." }
output "recovery_kms_key_arn" { value = aws_kms_key.recovery.arn }
output "recovery_kms_public_key_note" { value = "Legacy: the recovery key was every payment's recovery term before payments returned excess funds to the payer's own attested wallet. gatewayd no longer reads its address; keep it only until any balance it holds has been returned by hand." }
output "attestation_kms_key_arn" { value = aws_kms_key.attestation.arn }
output "attestation_signer_note" { value = "Derive the attestor address from this key with AWS_KMS_KEY_ID=<arn> cast wallet address --aws, verify it independently, and publish it as the trusted attestor merchants verify proofs against; only the API task role can sign with it and it never holds funds." }
output "attachment_bucket_name" { value = aws_s3_bucket.attachments.id }
output "database_url_secret_arn" { value = aws_secretsmanager_secret.database_url.arn }
output "rpc_url_secret_arn" { value = aws_secretsmanager_secret.rpc_url.arn }
output "webhook_encryption_key_secret_arn" { value = aws_secretsmanager_secret.webhook_encryption_key.arn }
output "admin_bearer_secret_arn" { value = aws_secretsmanager_secret.admin_bearer.arn }
output "ecs_cluster_name" { value = aws_ecs_cluster.this.name }
output "api_service_name" { value = aws_ecs_service.api.name }
output "indexer_service_name" { value = aws_ecs_service.indexer.name }
output "rds_endpoint" { value = aws_db_instance.this.endpoint }
output "notification_dkim_records" {
  description = "CNAME records to add in the DNS provider hosting notification_domain_name (Vercel for payday.sh); SES verifies the sending identity once they resolve."
  value = {
    for token in aws_sesv2_email_identity.notifications.dkim_signing_attributes[0].tokens :
    "${token}._domainkey.${var.notification_domain_name}" => "${token}.dkim.amazonses.com"
  }
}
