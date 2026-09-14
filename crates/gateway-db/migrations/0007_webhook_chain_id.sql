-- A merchant may now pin the network a request is paid on, and the webhook
-- payload is the API's subset, so it names the chain the way the API does:
-- the decimal chain id, null until it is known (issuance for a pinned
-- request, the wallet binding otherwise). The function body is the one in
-- 0001 plus that key.
CREATE OR REPLACE FUNCTION webhook_deposit_request_object(invoice invoices) RETURNS JSONB
LANGUAGE SQL STABLE AS $$
    SELECT jsonb_build_object(
        'id', 'dr_' || invoice.id::text,
        'status', webhook_public_status(invoice.status, invoice.confirmed_received, invoice.blocked_reason),
        'amount', webhook_decimal_amount(invoice.amount, invoice.token_decimals),
        'amount_base_units', invoice.amount,
        'received', webhook_decimal_amount(invoice.confirmed_received, invoice.token_decimals),
        'received_base_units', invoice.confirmed_received,
        'heading', invoice.heading,
        'reference', invoice.reference,
        'metadata', invoice.metadata,
        'customer_id', 'cus_' || invoice.customer_id::text,
        'issuer_id', 'iss_' || invoice.issuer_id::text,
        'payer_policy_mode', invoice.payer_policy_mode,
        'payer_reference', invoice.payer_reference,
        'verification_completed_at', webhook_rfc3339(invoice.verification_completed_at),
        'likely_unsolicited_at', webhook_rfc3339(invoice.likely_unsolicited_at),
        'payer_wallet', CASE WHEN invoice.payer_wallet IS NULL THEN NULL
                             ELSE '0x' || encode(invoice.payer_wallet, 'hex') END,
        'address', CASE WHEN invoice.payment_address IS NULL THEN NULL
                        ELSE '0x' || encode(invoice.payment_address, 'hex') END,
        'chain_id', CASE
            WHEN invoice.chain_id IS NOT NULL THEN invoice.chain_id::text
            WHEN jsonb_array_length(invoice.issuance_snapshot->'networks') = 1
                THEN invoice.issuance_snapshot->'networks'->0->>'chain_id'
            ELSE NULL END,
        'wallet_bound_at', webhook_rfc3339(invoice.wallet_bound_at),
        'expires_at', webhook_rfc3339(to_timestamp(invoice.expiration_timestamp)),
        'created_at', webhook_rfc3339(invoice.created_at))
$$;
