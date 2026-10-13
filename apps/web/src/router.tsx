import { Outlet, createRootRoute, createRoute, createRouter } from '@tanstack/react-router'
import { AppLayout } from './components/Layout'
import { AdminAccountsPage } from './pages/AdminAccounts'
import { AdminLoginsPage } from './pages/AdminLogins'
import { AdminSettingsPage } from './pages/AdminSettings'
import { LoginPage } from './pages/Login'
import { MyUsagePage } from './pages/MyUsage'

const rootRoute = createRootRoute({ component: () => <Outlet /> })
const loginRoute = createRoute({ getParentRoute: () => rootRoute, path: '/login', component: LoginPage })
const appRoute = createRoute({ getParentRoute: () => rootRoute, id: 'app', component: AppLayout })
const usageRoute = createRoute({ getParentRoute: () => appRoute, path: '/', component: MyUsagePage })
const loginsRoute = createRoute({ getParentRoute: () => appRoute, path: '/admin/logins', component: AdminLoginsPage })
const accountsRoute = createRoute({ getParentRoute: () => appRoute, path: '/admin/accounts', component: AdminAccountsPage })
const settingsRoute = createRoute({ getParentRoute: () => appRoute, path: '/admin/settings', component: AdminSettingsPage })

const routeTree = rootRoute.addChildren([
  loginRoute,
  appRoute.addChildren([usageRoute, loginsRoute, accountsRoute, settingsRoute]),
])

export const router = createRouter({ routeTree })

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}
