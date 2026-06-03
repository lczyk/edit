# terragrunt config -- exercises blocks, attrs, strings, interpolation
// alt line comment
/* block
   comment */

include "root" {
  path = find_in_parent_folders()
}

terraform {
  source = "git::git@github.com:org/modules.git//curl-rock?ref=v1.2.3"
}

dependency "vpc" {
  config_path = "../vpc"
}

locals {
  region   = "eu-west-2"
  replicas = 3
  enabled  = true
  fallback = null
  tags     = merge(local.common_tags, { Name = "curl-rock" })
}

inputs = {
  name        = "curl-${local.region}"
  vpc_id      = dependency.vpc.outputs.vpc_id
  user_data   = <<-EOT
    #!/bin/bash
    echo "hello ${local.region}"
  EOT
  cidr_blocks = [for s in var.subnets : s.cidr if s.public]
}
