# ACM certificate validation failure (CAA error)

ACM cannot issue a TLS certificate for `api.payday.sh` because the parent
domain's CAA records do not authorize Amazon as a certificate authority.

## Symptoms

Terraform apply fails with:

```
Error: waiting for ACM Certificate (...) to be issued: unexpected state 'FAILED',
wanted target 'ISSUED'. last error: CAA_ERROR
```

## Root cause

The parent domain `payday.sh` (hosted at Vercel or another registrar) has CAA
records that only authorize certain CAs (e.g. Let's Encrypt, Sectigo, Google).
ACM needs a CAA record authorizing `amazon.com` within the `api.payday.sh`
zone or the parent zone.

## Fix: Add CAA records in Route53

The `api.payday.sh` hosted zone is in Route53. Add two CAA records there:

```bash
ZONE_ID="<YOUR_ROUTE53_ZONE_ID>"

# Create a CAA record set JSON
cat > /tmp/caa-records.json << 'JSON'
{
  "Comment": "Allow Amazon to issue certificates for api.payday.sh",
  "Changes": [
    {
      "Action": "UPSERT",
      "ResourceRecordSet": {
        "Name": "api.payday.sh.",
        "Type": "CAA",
        "TTL": 300,
        "ResourceRecords": [
          {"Value": "0 issue \"amazon.com\""},
          {"Value": "0 issuewild \"amazon.com\""}
        ]
      }
    }
  ]
}
JSON

aws route53 change-resource-record-sets \
  --hosted-zone-id "$ZONE_ID" \
  --change-batch file:///tmp/caa-records.json \
  --region "$AWS_REGION"
```

## Verify CAA records propagated

```bash
dig CAA api.payday.sh +short
```

Should return:

```
0 issue "amazon.com"
0 issuewild "amazon.com"
```

DNS propagation may take a few minutes. If the records don't appear, check
that the NS delegation from the parent domain to Route53 is working:

```bash
dig NS api.payday.sh +short
```

## Retry the certificate

After CAA records are visible, taint the certificate resource and re-apply:

```bash
cd infra
terraform taint aws_acm_certificate.api
terraform apply
```

The certificate should now reach `ISSUED` status within a few minutes.

## Alternative: Add CAA records at the parent domain

If you control the parent `payday.sh` DNS, you can add the Amazon CAA records
there instead. At Vercel (or your registrar's DNS panel), add:

```
api.payday.sh. CAA 0 issue "amazon.com"
api.payday.sh. CAA 0 issuewild "amazon.com"
```

This works only if the `api` subdomain is delegated to Route53 — the CAA
records need to be resolvable at the parent level for ACM to find them.
