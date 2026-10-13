import { ArrowLeft } from 'lucide-react'
import { Link, useNavigate } from 'react-router'
// Desktop: account reset when the trader wants to start over completely.
import { ResetAccountCard } from '@/components/auth/ResetAccountCard'
// Desktop: a forgotten password is replaced by setting a new one.
import { StartFreshCard } from '@/components/auth/StartFreshCard'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { showToast } from '@/utils/toast'

/**
 * Desktop departure from the web page (KEY-01, maintainer decision
 * 2026-10-13). The web resets a password with an email and an authenticator
 * code. OpenAlgo Desktop locks the authenticator secret, saved broker keys
 * and the API key with the password itself, so nothing can check a code or
 * open those values while the password is lost. A new password keeps the
 * account, settings and history, and the locked values are set up again.
 */
export default function ResetPassword() {
  const navigate = useNavigate()

  const onDone = (message: string) => {
    showToast.success(message, 'system')
    navigate('/login', { replace: true })
  }

  return (
    <div className="min-h-[calc(100vh-8rem)] flex items-center justify-center p-4">
      <div className="w-full max-w-lg">
        <Card>
          <CardHeader>
            <CardTitle className="text-2xl">Reset Password</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <StartFreshCard reason="forgot" onDone={onDone} />

            <div className="text-center">
              <Link
                to="/login"
                className="text-sm text-primary hover:underline inline-flex items-center gap-1"
              >
                <ArrowLeft className="h-4 w-4" />
                Back to Login
              </Link>
            </div>

            <ResetAccountCard />
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
