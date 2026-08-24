data "aws_availability_zones" "available" { state = "available" }

locals {
  azs = slice(data.aws_availability_zones.available.names, 0, 2)
  common_environment = [
    { name = "GATEWAY_CHAIN_ID", value = tostring(var.chain_id) },
    { name = "GATEWAY_FACTORY_ADDRESS", value = var.factory_address },
    { name = "GATEWAY_USDC_ADDRESS", value = var.usdc_address },
    { name = "RUST_LOG", value = "info" }
  ]
}

resource "aws_vpc" "this" {
  cidr_block           = "10.42.0.0/16"
  enable_dns_hostnames = true
  enable_dns_support   = true
}

resource "aws_internet_gateway" "this" { vpc_id = aws_vpc.this.id }

resource "aws_subnet" "public" {
  count                   = 2
  vpc_id                  = aws_vpc.this.id
  availability_zone       = local.azs[count.index]
  cidr_block              = cidrsubnet(aws_vpc.this.cidr_block, 8, count.index)
  map_public_ip_on_launch = true
}

resource "aws_subnet" "private" {
  count             = 2
  vpc_id            = aws_vpc.this.id
  availability_zone = local.azs[count.index]
  cidr_block        = cidrsubnet(aws_vpc.this.cidr_block, 8, count.index + 10)
}

resource "aws_route_table" "public" { vpc_id = aws_vpc.this.id }
resource "aws_route" "internet" {
  route_table_id         = aws_route_table.public.id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.this.id
}
resource "aws_route_table_association" "public" {
  count          = 2
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

resource "aws_security_group" "alb" {
  name_prefix = "${var.name}-alb-"
  vpc_id      = aws_vpc.this.id
  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "api" {
  name_prefix = "${var.name}-api-"
  vpc_id      = aws_vpc.this.id
  ingress {
    from_port       = var.api_port
    to_port         = var.api_port
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "indexer" {
  name_prefix = "${var.name}-indexer-"
  vpc_id      = aws_vpc.this.id
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "db" {
  name_prefix = "${var.name}-db-"
  vpc_id      = aws_vpc.this.id
  ingress {
    from_port       = 5432
    to_port         = 5432
    protocol        = "tcp"
    security_groups = [aws_security_group.api.id, aws_security_group.indexer.id]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "random_password" "db" {
  length  = 32
  special = false
}
resource "random_password" "api_key" {
  length  = 48
  special = false
}

resource "aws_db_subnet_group" "this" {
  name       = var.name
  subnet_ids = aws_subnet.private[*].id
}

resource "aws_db_instance" "this" {
  identifier                   = var.name
  engine                       = "postgres"
  engine_version               = "16"
  instance_class               = var.db_instance_class
  allocated_storage            = var.db_allocated_storage
  max_allocated_storage        = var.db_max_allocated_storage
  storage_type                 = "gp3"
  storage_encrypted            = true
  db_name                      = "gateway"
  username                     = "gateway"
  password                     = random_password.db.result
  port                         = 5432
  multi_az                     = var.db_multi_az
  publicly_accessible          = false
  deletion_protection          = var.db_deletion_protection
  skip_final_snapshot          = var.db_skip_final_snapshot
  final_snapshot_identifier    = var.db_skip_final_snapshot ? null : "${var.name}-final"
  backup_retention_period      = 7
  auto_minor_version_upgrade   = true
  db_subnet_group_name         = aws_db_subnet_group.this.name
  vpc_security_group_ids       = [aws_security_group.db.id]
  performance_insights_enabled = true
}

resource "aws_secretsmanager_secret" "database_url" { name = "${var.name}/database-url" }
resource "aws_secretsmanager_secret_version" "database_url" {
  secret_id     = aws_secretsmanager_secret.database_url.id
  secret_string = "postgresql://gateway:${random_password.db.result}@${aws_db_instance.this.address}:5432/gateway?sslmode=verify-full&sslrootcert=/usr/local/share/ca-certificates/aws-rds-global-bundle.pem"
}
resource "aws_secretsmanager_secret" "api_key" { name = "${var.name}/api-key" }
resource "aws_secretsmanager_secret_version" "api_key" {
  secret_id     = aws_secretsmanager_secret.api_key.id
  secret_string = random_password.api_key.result
}
resource "aws_secretsmanager_secret" "rpc_url" { name = "${var.name}/rpc-url" }
resource "aws_secretsmanager_secret_version" "rpc_url" {
  secret_id     = aws_secretsmanager_secret.rpc_url.id
  secret_string = var.rpc_url
}

resource "aws_kms_key" "signer" {
  description              = "${var.name} Ethereum transaction signer"
  key_usage                = "SIGN_VERIFY"
  customer_master_key_spec = "ECC_SECG_P256K1"
  deletion_window_in_days  = 30
  enable_key_rotation      = false
  lifecycle {
    prevent_destroy = true
  }
}
resource "aws_kms_alias" "signer" {
  name          = "alias/${var.name}-signer"
  target_key_id = aws_kms_key.signer.key_id
}

resource "aws_ecr_repository" "api" {
  name                 = "${var.name}-api"
  image_tag_mutability = "IMMUTABLE"
  image_scanning_configuration { scan_on_push = true }
}
resource "aws_ecr_repository" "indexer" {
  name                 = "${var.name}-indexer"
  image_tag_mutability = "IMMUTABLE"
  image_scanning_configuration { scan_on_push = true }
}

resource "aws_cloudwatch_log_group" "api" {
  name              = "/ecs/${var.name}/api"
  retention_in_days = 30
}
resource "aws_cloudwatch_log_group" "indexer" {
  name              = "/ecs/${var.name}/indexer"
  retention_in_days = 30
}

resource "aws_ecs_cluster" "this" {
  name = var.name
  setting {
    name  = "containerInsights"
    value = "enabled"
  }
}

resource "aws_iam_role" "api_execution" {
  name               = "${var.name}-api-execution"
  assume_role_policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Principal = { Service = "ecs-tasks.amazonaws.com" }, Action = "sts:AssumeRole" }] })
}
resource "aws_iam_role" "indexer_execution" {
  name               = "${var.name}-indexer-execution"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role_policy_attachment" "api_execution" {
  role       = aws_iam_role.api_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}
resource "aws_iam_role_policy_attachment" "indexer_execution" {
  role       = aws_iam_role.indexer_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "api_secrets" {
  role   = aws_iam_role.api_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = [aws_secretsmanager_secret.database_url.arn, aws_secretsmanager_secret.api_key.arn] }] })
}
resource "aws_iam_role_policy" "indexer_secrets" {
  role   = aws_iam_role.indexer_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = [aws_secretsmanager_secret.database_url.arn, aws_secretsmanager_secret.rpc_url.arn] }] })
}

resource "aws_iam_role" "api_task" {
  name               = "${var.name}-api-task"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role" "indexer_task" {
  name               = "${var.name}-indexer-task"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role_policy" "indexer_kms" {
  role   = aws_iam_role.indexer_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["kms:GetPublicKey", "kms:Sign"], Resource = aws_kms_key.signer.arn }] })
}

resource "aws_ecs_task_definition" "api" {
  family                   = "${var.name}-api"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.api_cpu
  memory                   = var.api_memory
  execution_role_arn       = aws_iam_role.api_execution.arn
  task_role_arn            = aws_iam_role.api_task.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "api", image = "${aws_ecr_repository.api.repository_url}:${var.image_tag}", essential = true,
    readonlyRootFilesystem = true,
    portMappings           = [{ containerPort = var.api_port, protocol = "tcp" }],
    environment            = concat(local.common_environment, [{ name = "GATEWAY_BIND_ADDR", value = "0.0.0.0:${var.api_port}" }]),
    secrets                = [{ name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn }, { name = "GATEWAY_API_KEY", valueFrom = aws_secretsmanager_secret.api_key.arn }],
    logConfiguration       = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.api.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "api" } }
  }])
}

resource "aws_ecs_task_definition" "indexer" {
  family                   = "${var.name}-indexer"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.indexer_cpu
  memory                   = var.indexer_memory
  execution_role_arn       = aws_iam_role.indexer_execution.arn
  task_role_arn            = aws_iam_role.indexer_task.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "indexer", image = "${aws_ecr_repository.indexer.repository_url}:${var.image_tag}", essential = true,
    readonlyRootFilesystem = true,
    stopTimeout            = 120,
    environment = concat(local.common_environment, [
      { name = "GATEWAY_KMS_KEY_ID", value = aws_kms_key.signer.arn },
      { name = "GATEWAY_USDC_START_BLOCK", value = tostring(var.usdc_start_block) },
      { name = "GATEWAY_FINALITY_CONFIRMATIONS", value = tostring(var.finality_confirmations) },
      { name = "GATEWAY_LOG_RANGE_SIZE", value = tostring(var.log_range_size) }
    ]),
    secrets          = [{ name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn }, { name = "GATEWAY_RPC_URL", valueFrom = aws_secretsmanager_secret.rpc_url.arn }],
    logConfiguration = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.indexer.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "indexer" } }
  }])
}

resource "aws_lb" "api" {
  name               = var.name
  load_balancer_type = "application"
  subnets            = aws_subnet.public[*].id
  security_groups    = [aws_security_group.alb.id]
}
resource "aws_lb_target_group" "api" {
  name        = "${var.name}-api"
  port        = var.api_port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.this.id
  health_check {
    path    = "/health"
    matcher = "200-399"
  }
}

resource "aws_acm_certificate" "api" {
  domain_name       = var.domain_name
  validation_method = "DNS"
  lifecycle { create_before_destroy = true }
}
resource "aws_route53_record" "validation" {
  for_each = { for dvo in aws_acm_certificate.api.domain_validation_options : dvo.domain_name => { name = dvo.resource_record_name, record = dvo.resource_record_value, type = dvo.resource_record_type } }
  zone_id  = var.route53_zone_id
  name     = each.value.name
  type     = each.value.type
  records  = [each.value.record]
  ttl      = 60
}
resource "aws_acm_certificate_validation" "api" {
  certificate_arn         = aws_acm_certificate.api.arn
  validation_record_fqdns = [for r in aws_route53_record.validation : r.fqdn]
}
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.api.arn
  port              = 80
  protocol          = "HTTP"
  default_action {
    type = "redirect"
    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}
resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.api.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = aws_acm_certificate_validation.api.certificate_arn
  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.api.arn
  }
}
resource "aws_route53_record" "api" {
  zone_id = var.route53_zone_id
  name    = var.domain_name
  type    = "A"
  alias {
    name                   = aws_lb.api.dns_name
    zone_id                = aws_lb.api.zone_id
    evaluate_target_health = true
  }
}

resource "aws_ecs_service" "api" {
  name            = "api"
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.api.arn
  desired_count   = var.api_desired_count
  launch_type     = "FARGATE"
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }
  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.api.id]
    assign_public_ip = true
  }
  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
    container_name   = "api"
    container_port   = var.api_port
  }
  depends_on = [aws_lb_listener.https]
}
resource "aws_ecs_service" "indexer" {
  name                               = "indexer"
  cluster                            = aws_ecs_cluster.this.id
  task_definition                    = aws_ecs_task_definition.indexer.arn
  desired_count                      = 1
  launch_type                        = "FARGATE"
  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }
  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.indexer.id]
    assign_public_ip = true
  }
}

resource "aws_wafv2_web_acl" "api" {
  name  = var.name
  scope = "REGIONAL"
  default_action {
    allow {}
  }
  rule {
    name     = "ip-rate-limit"
    priority = 1
    action {
      block {}
    }
    statement {
      rate_based_statement {
        limit              = var.waf_rate_limit
        aggregate_key_type = "IP"
      }
    }
    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-rate-limit"
      sampled_requests_enabled   = false
    }
  }
  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = var.name
    sampled_requests_enabled   = false
  }
}
resource "aws_wafv2_web_acl_association" "api" {
  resource_arn = aws_lb.api.arn
  web_acl_arn  = aws_wafv2_web_acl.api.arn
}

resource "aws_sns_topic" "alarms" {
  name = "${var.name}-alarms"
}

resource "aws_sns_topic_subscription" "alarm_email" {
  topic_arn = aws_sns_topic.alarms.arn
  protocol  = "email"
  endpoint  = var.alarm_email
}

resource "aws_cloudwatch_metric_alarm" "unhealthy" {
  alarm_name          = "${var.name}-unhealthy-targets"
  namespace           = "AWS/ApplicationELB"
  metric_name         = "UnHealthyHostCount"
  statistic           = "Maximum"
  period              = 60
  evaluation_periods  = 2
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
  dimensions          = { LoadBalancer = aws_lb.api.arn_suffix, TargetGroup = aws_lb_target_group.api.arn_suffix }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "api_tasks" {
  alarm_name          = "${var.name}-api-task-count"
  namespace           = "ECS/ContainerInsights"
  metric_name         = "RunningTaskCount"
  statistic           = "Minimum"
  period              = 60
  evaluation_periods  = 2
  threshold           = var.api_desired_count
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching"
  dimensions          = { ClusterName = aws_ecs_cluster.this.name, ServiceName = aws_ecs_service.api.name }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "indexer_tasks" {
  alarm_name          = "${var.name}-indexer-task-count"
  namespace           = "ECS/ContainerInsights"
  metric_name         = "RunningTaskCount"
  statistic           = "Minimum"
  period              = 60
  evaluation_periods  = 2
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching"
  dimensions          = { ClusterName = aws_ecs_cluster.this.name, ServiceName = aws_ecs_service.indexer.name }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_log_metric_filter" "indexer_fatal" {
  name           = "${var.name}-indexer-fatal"
  log_group_name = aws_cloudwatch_log_group.indexer.name
  pattern        = "\"indexer fatal\""
  metric_transformation {
    name      = "IndexerFatal"
    namespace = var.name
    value     = "1"
  }
}
resource "aws_cloudwatch_metric_alarm" "indexer_fatal" {
  alarm_name          = "${var.name}-indexer-fatal"
  namespace           = var.name
  metric_name         = "IndexerFatal"
  statistic           = "Sum"
  period              = 60
  evaluation_periods  = 1
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "db_cpu" {
  alarm_name          = "${var.name}-db-high-cpu"
  namespace           = "AWS/RDS"
  metric_name         = "CPUUtilization"
  statistic           = "Average"
  period              = 300
  evaluation_periods  = 3
  threshold           = 80
  comparison_operator = "GreaterThanThreshold"
  dimensions          = { DBInstanceIdentifier = aws_db_instance.this.identifier }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "db_storage" {
  alarm_name          = "${var.name}-db-low-storage"
  namespace           = "AWS/RDS"
  metric_name         = "FreeStorageSpace"
  statistic           = "Minimum"
  period              = 300
  evaluation_periods  = 1
  threshold           = 5368709120
  comparison_operator = "LessThanThreshold"
  dimensions          = { DBInstanceIdentifier = aws_db_instance.this.identifier }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
