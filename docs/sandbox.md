# Sandbox environment

The sandbox is a second, isolated instance of the production Terraform
architecture at `api.sandbox.payday.sh`. It uses Monad testnet (chain ID
`10143`) and Circle test USDC (`0xf817257fed379853cDe0fa4F97AB987181B1E5Ea`).
It must have separate Auth0 resources, database, signer, alarms, and Terraform
state; it is not a workspace layered onto production state.

## Plan the second deployment

No infrastructure is deployed by these instructions. Copy
`infra/terraform.sandbox.tfvars.example` to an ignored tfvars file, replace the
example image/Auth0/Route53 values, and insert the factory, batch sweeper, and
start block from the actual testnet contract deployment. Use a distinct backend:

```hcl
bucket       = "company-payday-terraform-state"
key          = "sandbox/payday.tfstate"
region       = "us-east-1"
encrypt      = true
use_lockfile = true
```

Then inspect, rather than automatically apply, the plan:

```bash
export TF_VAR_rpc_url='https://your-testnet-provider.example/...'
terraform -chdir=infra init -backend-config=backend.sandbox.hcl
terraform -chdir=infra plan -var-file=terraform.sandbox.tfvars -out=sandbox.tfplan
```

The distinct backend state key and `name = "payday-sandbox"` cause Terraform
to instantiate a separate VPC, database, ECS cluster/services, repositories,
secrets, signer, load balancer, alarms, and DNS record. `api_key_prefix =
"payday_test_"` makes this service issue sandbox keys; production explicitly
uses `payday_live_`.

## Calling it

Point the SDK or `curl` at the sandbox origin with a `payday_test_` key:

```bash
curl -fsS "https://api.sandbox.payday.sh/v1/payments/<PAYMENT_ID>" \
  -H "Authorization: Bearer payday_test_..."
```

In the SDK, pass `baseUrl: "https://api.sandbox.payday.sh"` to `PaydayClient`.
