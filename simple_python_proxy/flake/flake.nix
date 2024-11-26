{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs = {
    self,
    nixpkgs,
  }: {
    packages.x86_64-linux.hello = nixpkgs.legacyPackages.x86_64-linux.fetchurl {
      url = "https://github.com/TyberiusPrime/dppd/commit/e5fc8853289efa79ff278effd14f89e1c652b6b6";
      sha256 = "sha256-s9JX7YiNODRS0GNim0ALFf5lxzV1shFOXPHQUapH+WM=";
    };

    packages.x86_64-linux.default = self.packages.x86_64-linux.hello;
  };
}
