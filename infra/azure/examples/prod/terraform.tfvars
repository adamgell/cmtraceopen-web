###############################################################################
# Example tfvars — copy to terraform.auto.tfvars (or pass via CI secret store).
# DO NOT commit your real values; this file holds placeholders.
###############################################################################

entra_tenant_id              = "00000000-0000-0000-0000-000000000000"
entra_audience               = "api://cmtrace-api"
kv_admin_group_object_id     = "11111111-1111-1111-1111-111111111111"
pg_aad_admin_group_object_id = "22222222-2222-2222-2222-222222222222"
