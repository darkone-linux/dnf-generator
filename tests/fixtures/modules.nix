{
  adguardhome = { uniquePerZone = true; };
  headscale = { reverseProxy = false; externalAccess = true; };
  homepage = { uniquePerZone = true; externalAccess = true; };
  restic = { reverseProxy = false; };
}
