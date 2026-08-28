terraform {
  required_version = ">= 1.10.0, < 2.0.0"

  required_providers {
    auth0 = {
      source  = "auth0/auth0"
      version = "~> 1.56.0"
    }
  }

  # Keep Auth0 state separate from the AWS stack. Configure this at init time.
  backend "s3" {}
}

provider "auth0" {}
