package auth

import "strings"

type Allowlist struct {
	domains map[string]bool
	emails  map[string]bool
}

func NewAllowlist(domains, emails []string) Allowlist {
	a := Allowlist{domains: map[string]bool{}, emails: map[string]bool{}}
	for _, d := range domains {
		a.domains[strings.ToLower(strings.TrimPrefix(d, "@"))] = true
	}
	for _, e := range emails {
		a.emails[strings.ToLower(e)] = true
	}
	return a
}

// Allows accepts verified emails listed explicitly, or Google Workspace
// accounts whose hd claim is an allowed domain. The email's domain alone is
// not trusted: consumer accounts can use any address.
func (a Allowlist) Allows(id Identity) bool {
	if !id.EmailVerified {
		return false
	}
	if a.emails[strings.ToLower(id.Email)] {
		return true
	}
	hd := strings.ToLower(id.HostedDomain)
	return hd != "" && a.domains[hd]
}
