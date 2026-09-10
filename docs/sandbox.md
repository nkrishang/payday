# Sandbox environment

The sandbox is a second, isolated instance of the production Terraform
architecture at `api.sandbox.payday.sh`. It offers the testnets of the
production networks with Circle's test USDC: Monad testnet (chain ID `10143`,
`0xf817257fed379853cDe0fa4F97AB987181B1E5Ea`), Base Sepolia (`84532`,
`0x036CbD53842c5426634e7929541eC2318f3dCF7e`), and Arbitrum Sepolia
(`421614`, `0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d`); verify each against
Circle's contract-address page before applying.
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
export TF_VAR_rpc_urls='{"10143":"https://…","84532":"https://…","421614":"https://…"}'
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
curl -fsS "https://api.sandbox.payday.sh/v1/deposit-requests/<DEPOSIT_REQUEST_ID>" \
  -H "Authorization: Bearer payday_test_..."
```

In the SDK, pass `baseUrl: "https://api.sandbox.payday.sh"` to `PaydayClient`.
