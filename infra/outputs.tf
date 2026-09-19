output "api_url" { value = "https://${var.domain_name}" }
output "api_ecr_repository_url" { value = aws_ecr_repository.api.repository_url }
output "indexer_ecr_repository_url" { value = aws_ecr_repository.indexer.repository_url }
output "signers_ecr_repository_url" { value = aws_ecr_repository.signers.repository_url }
output "migrate_task_definition" {
  description = "Run with `aws ecs run-task` and wait for it before rolling the services; it applies pending schema migrations with the new image."
  value       = aws_ecs_task_definition.migrate.family
}
output "kms_key_arns" { value = aws_kms_key.signer[*].arn }
output "kms_public_key_note" { value = "The sweep signer pool: AWS KMS exposes each key's secp256k1 public key via GetPublicKey, not an Ethereum address; derive and independently verify every address before funding it on every chain." }
output "recovery_kms_key_arn" { value = aws_kms_key.recovery.arn }
output "recovery_kms_public_key_note" { value = "Gum's custody wallet: derive its Ethereum address (AWS_KMS_KEY_ID=<arn> cast wallet address --aws, verify independently), set it as the recovery_address variable, and never grant a task role kms:Sign with it — recovered funds are returned by hand, signing out of band." }
output "attestation_kms_key_arn" { value = aws_kms_key.attestation.arn }
output "attestation_signer_note" { value = "Derive the attestor address from this key with AWS_KMS_KEY_ID=<arn> cast wallet address --aws, verify it independently, and publish it as the trusted attestor merchants verify proofs against; only the API task role can sign with it and it never holds funds." }
output "attachment_bucket_name" { value = aws_s3_bucket.attachments.id }
output "database_url_secret_arn" { value = aws_secretsmanager_secret.database_url.arn }
output "rpc_url_secret_arns" { value = { for id, secret in aws_secretsmanager_secret.rpc_url : id => secret.arn } }
output "webhook_encryption_key_secret_arn" { value = aws_secretsmanager_secret.webhook_encryption_key.arn }
output "admin_bearer_secret_arn" { value = aws_secretsmanager_secret.admin_bearer.arn }
output "ecs_cluster_name" { value = aws_ecs_cluster.this.name }
output "api_service_name" { value = aws_ecs_service.api.name }
output "indexer_service_name" { value = aws_ecs_service.indexer.name }
output "signers_service_name" { value = aws_ecs_service.signers.name }
output "indexer_security_group_id" { value = aws_security_group.indexer.id }
output "private_subnet_ids" { value = aws_subnet.private[*].id }
output "public_subnet_ids" { value = aws_subnet.public[*].id }
output "api_security_group_id" { value = aws_security_group.api.id }
output "rds_endpoint" { value = aws_db_instance.this.endpoint }
output "notification_dkim_records" {
  description = "CNAME records to add in the DNS provider hosting notification_domain_name (Vercel for gum.money); SES verifies the sending identity once they resolve."
  value = {
    for token in aws_sesv2_email_identity.notifications.dkim_signing_attributes[0].tokens :
    "${token}._domainkey.${var.notification_domain_name}" => "${token}.dkim.amazonses.com"
  }
}
output "github_deploy_role_arn" {
  description = "Role the deploy-staging workflow assumes; null unless github_repository is set."
  value       = try(aws_iam_role.github_deploy[0].arn, null)
}
output "image_tag" {
  description = "The git-<sha> tag currently applied; the deploy-staging workflow diffs the migrations directory against it."
  value       = var.image_tag
}
